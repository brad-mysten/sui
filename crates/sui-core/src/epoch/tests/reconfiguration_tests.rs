// Copyright (c) 2022, Mysten Labs, Inc.
// SPDX-License-Identifier: Apache-2.0

use crate::authority::{AuthorityState, AuthorityStore};
use crate::authority_client::NetworkAuthorityClient;
use crate::authority_server::AuthorityServer;
use crate::checkpoints::CheckpointStore;
use crate::{
    authority::AuthorityTemporaryStore, authority_active::ActiveAuthority,
    authority_aggregator::authority_aggregator_tests::init_local_authorities,
    checkpoints::CheckpointLocals, epoch::reconfiguration::CHECKPOINT_COUNT_PER_EPOCH,
    execution_engine,
};
use parking_lot::Mutex;
use sui_types::committee::Committee;
use test_utils::messages::move_transaction;

use crate::{gateway_state::GatewayMetrics, transaction_input_checker::InputObjects};
use std::{
    collections::{BTreeMap, BTreeSet},
    env, fs,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};
use sui_types::{
    base_types::{ObjectID, SuiAddress},
    crypto::{get_key_pair, AuthoritySignature, Signature},
    error::SuiError,
    gas::SuiGasStatus,
    messages::{ConfirmationTransaction, SignatureAggregator, Transaction, TransactionData},
    object::Object,
    SUI_SYSTEM_STATE_OBJECT_ID,
};

#[tokio::test]
async fn test_start_epoch_change() {
    // Create a sender, owning an object and a gas object.
    let (sender, sender_key) = get_key_pair();
    let object = Object::with_id_owner_for_testing(ObjectID::random(), sender);
    let gas_object = Object::with_id_owner_for_testing(ObjectID::random(), sender);
    let genesis_objects = vec![object.clone(), gas_object.clone()];
    // Create authority_aggregator and authority states.
    let (net, states) = init_local_authorities(vec![
        genesis_objects.clone(),
        genesis_objects.clone(),
        genesis_objects.clone(),
        genesis_objects.clone(),
    ])
    .await;
    let state = states[0].clone();
    // Set the checkpoint number to be near the end of epoch.
    state
        .checkpoints
        .as_ref()
        .unwrap()
        .lock()
        .set_locals_for_testing(CheckpointLocals {
            next_checkpoint: CHECKPOINT_COUNT_PER_EPOCH - 1,
            proposal_next_transaction: None,
            next_transaction_sequence: 0,
            no_more_fragments: true,
            current_proposal: None,
        })
        .unwrap();
    // Create an active authority for the first authority state.
    let active = ActiveAuthority::new_with_ephemeral_follower_store(
        state.clone(),
        net.clone_inner_clients(),
        GatewayMetrics::new_for_tests(),
    )
    .unwrap();
    // Make the high watermark differ from low watermark.
    let ticket = state.batch_notifier.ticket().unwrap();

    // Invoke start_epoch_change on the active authority.
    let epoch_change_started = Arc::new(AtomicBool::new(false));
    let epoch_change_started_copy = epoch_change_started.clone();
    let handle = tokio::spawn(async move {
        active.start_epoch_change().await.unwrap();
        epoch_change_started_copy.store(true, Ordering::SeqCst);
    });
    tokio::time::sleep(Duration::from_millis(100)).await;
    // Validator should now be halted, but epoch change hasn't finished.
    assert!(state.halted.load(Ordering::SeqCst));
    assert!(!epoch_change_started.load(Ordering::SeqCst));

    // Drain ticket.
    drop(ticket);
    tokio::time::sleep(Duration::from_millis(100)).await;
    // After we drained ticket, epoch change started.
    assert!(epoch_change_started.load(Ordering::SeqCst));
    handle.await.unwrap();

    // Test that when validator is halted, we cannot send any transaction.
    let tx_data = TransactionData::new_transfer(
        SuiAddress::default(),
        object.compute_object_reference(),
        sender,
        gas_object.compute_object_reference(),
        1000,
    );
    let signature = Signature::new(&tx_data, &sender_key);
    let transaction = Transaction::new(tx_data, signature);
    assert_eq!(
        state
            .handle_transaction(transaction.clone())
            .await
            .unwrap_err(),
        SuiError::ValidatorHaltedAtEpochEnd
    );

    // Test that when validator is halted, we cannot send any certificate.
    let mut sigs = SignatureAggregator::try_new(transaction.clone(), &net.committee).unwrap();
    let mut cert = None;
    for state in &states {
        cert = sigs
            .append(
                state.name,
                AuthoritySignature::new(&transaction.data, &*state.secret),
            )
            .unwrap();
    }
    let certificate = cert.unwrap();
    assert_eq!(
        state
            .handle_confirmation_transaction(ConfirmationTransaction {
                certificate: certificate.clone()
            })
            .await
            .unwrap_err(),
        SuiError::ValidatorHaltedAtEpochEnd
    );

    // Test that for certificates that have finished execution and is about to write effects,
    // they will also fail to get a ticket for the commit.
    let tx_digest = *transaction.digest();
    let mut temporary_store = AuthorityTemporaryStore::new(
        state.database.clone(),
        InputObjects::new(
            transaction
                .data
                .input_objects()
                .unwrap()
                .into_iter()
                .zip(genesis_objects)
                .collect(),
        ),
        tx_digest,
    );
    let (effects, _) = execution_engine::execute_transaction_to_effects(
        vec![],
        &mut temporary_store,
        transaction.data.clone(),
        tx_digest,
        BTreeSet::new(),
        &state.move_vm,
        &state._native_functions,
        SuiGasStatus::new_with_budget(1000, 1, 1),
        state.committee.load().epoch,
    );
    let signed_effects = effects.to_sign_effects(0, &state.name, &*state.secret);
    assert_eq!(
        state
            .commit_certificate(temporary_store, &certificate, &signed_effects)
            .await
            .unwrap_err(),
        SuiError::ValidatorHaltedAtEpochEnd
    );
}

#[tokio::test]
async fn test_finish_epoch_change() {
    // Create authority_aggregator and authority states.
    let genesis_objects = vec![];
    let (net, states) = init_local_authorities(vec![
        genesis_objects.clone(),
        genesis_objects.clone(),
        genesis_objects.clone(),
        genesis_objects.clone(),
    ])
    .await;
    let actives: Vec<_> = states
        .iter()
        .map(|state| {
            ActiveAuthority::new_with_ephemeral_follower_store(
                state.clone(),
                net.clone_inner_clients(),
                GatewayMetrics::new_for_tests(),
            )
            .unwrap()
        })
        .collect();
    let results: Vec<_> = states
        .iter()
        .zip(actives.iter())
        .map(|(state, active)| {
            async {
                // Set the checkpoint number to be near the end of epoch.
                let mut locals = CheckpointLocals {
                    next_checkpoint: CHECKPOINT_COUNT_PER_EPOCH - 1,
                    proposal_next_transaction: None,
                    next_transaction_sequence: 0,
                    no_more_fragments: true,
                    current_proposal: None,
                };
                state
                    .checkpoints
                    .as_ref()
                    .unwrap()
                    .lock()
                    .set_locals_for_testing(locals.clone())
                    .unwrap();

                active.start_epoch_change().await.unwrap();

                locals.next_checkpoint += 1;
                state
                    .checkpoints
                    .as_ref()
                    .unwrap()
                    .lock()
                    .set_locals_for_testing(locals.clone())
                    .unwrap();

                active.finish_epoch_change().await.unwrap()
            }
        })
        .collect();
    futures::future::join_all(results).await;

    // Verify that epoch changed in every authority state.
    for active in actives {
        assert_eq!(active.state.committee.load().epoch, 1);
        assert_eq!(active.net.load().committee.epoch, 1);
        assert_eq!(
            active
                .state
                .db()
                .get_last_epoch_info()
                .unwrap()
                .committee
                .epoch,
            1
        );
        // Verify that validator is no longer halted.
        assert!(!active.state.halted.load(Ordering::SeqCst));
        let system_state = active.state.get_sui_system_state_object().await.unwrap();
        assert_eq!(system_state.epoch, 1);
        let (_, tx_digest) = active
            .state
            .get_latest_parent_entry(SUI_SYSTEM_STATE_OBJECT_ID)
            .await
            .unwrap()
            .unwrap();
        let response = active
            .state
            .handle_transaction_info_request(tx_digest.into())
            .await
            .unwrap();
        assert!(response.signed_effects.is_some());
        assert!(response.certified_transaction.is_some());
        assert!(response.signed_effects.is_some());
    }
}
use crate::authority_aggregator::AuthorityAggregator;
use test_utils::objects::test_gas_objects;

#[tokio::test]
async fn test_epoch_change_committee_updates() {
    let (net, states): (
        AuthorityAggregator<NetworkAuthorityClient>,
        Vec<Arc<AuthorityState>>,
    ) = init_network_authorities(4).await;

    let _actives: Vec<_> = states
        .iter()
        .map(|state| {
            let result = ActiveAuthority::new_with_ephemeral_follower_store(
                state.clone(),
                net.clone_inner_clients(),
                GatewayMetrics::new_for_tests(),
            );

            result.unwrap()
        })
        .collect();

    let mut gas_objects = test_gas_objects();
    let _transaction = move_transaction(
        gas_objects.pop().unwrap(),
        "sui_system",
        "request_add_validator",
        gas_objects.pop().unwrap().compute_object_reference(),
        vec![], // TODO
    );
}
pub async fn init_network_authorities(
    num_authorities: usize,
) -> (
    AuthorityAggregator<NetworkAuthorityClient>,
    Vec<Arc<AuthorityState>>,
) {
    let mut key_pairs = Vec::new();
    let mut voting_rights = BTreeMap::new();
    for _ in 0..num_authorities {
        let (_, key_pair) = get_key_pair();
        let authority_name = *key_pair.public_key_bytes();
        voting_rights.insert(authority_name, 1);
        key_pairs.push((authority_name, key_pair));
    }
    let committee = Committee::new(0, voting_rights).unwrap();

    let mut clients = BTreeMap::new();
    let mut states = Vec::new();

    let mut i = 0;
    for (authority_name, secret) in key_pairs.into_iter() {
        let secret = Arc::pin(secret);
        // create store at random directory
        let dir = env::temp_dir();
        let path = dir.join(format!("DB_{:?}", ObjectID::random()));
        fs::create_dir(&path).unwrap();
        let mut store_path = path.clone();
        store_path.push("store");
        let store = Arc::new(AuthorityStore::open(&store_path, None));
        let mut checkpoints_path = path.clone();
        checkpoints_path.push("checkpoints");
        let checkpoints = CheckpointStore::open(
            &checkpoints_path,
            None,
            committee.epoch,
            authority_name,
            secret.clone(),
        )
        .expect("Should not fail to open local checkpoint DB");

        let state = Arc::new(
            AuthorityState::new(
                committee.clone(),
                authority_name,
                secret.clone(),
                store,
                None,
                None,
                Some(Arc::new(Mutex::new(checkpoints))),
                &sui_config::genesis::Genesis::get_default_genesis(),
                &prometheus::Registry::new(),
            )
            .await,
        );
        // The following two fields are only needed for shared objects.
        let consensus_address = "/ip4/127.0.0.1/tcp/0/http".parse().unwrap();
        let (tx_consensus_listener, _rx_consensus_listener) = tokio::sync::mpsc::channel(1);

        let server = AuthorityServer::new(
            format!("/ip4/127.0.0.1/tcp/10{}/http", i).parse().unwrap(),
            state.clone(),
            consensus_address,
            tx_consensus_listener,
        );
        i += 1;

        let res = server.spawn().await;
        let server_handle = res.unwrap();

        let client = NetworkAuthorityClient::connect(server_handle.address())
            .await
            .unwrap();

        states.push(state.clone());
        clients.insert(authority_name, client);
    }
    let aggregator = AuthorityAggregator::new(committee, clients, GatewayMetrics::new_for_tests());

    (aggregator, states)
}
