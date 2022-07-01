// Copyright (c) 2022, Mysten Labs, Inc.
// SPDX-License-Identifier: Apache-2.0

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use sui_core::authority_aggregator::AuthorityAggregator;
use sui_core::authority_client::NetworkAuthorityClient;
use sui_node::SuiNode;
use sui_quorum_driver::QuorumDriverHandler;
use sui_types::base_types::SuiAddress;
use sui_types::messages::{
    ExecuteTransactionRequest, ExecuteTransactionRequestType, ExecuteTransactionResponse,
    Transaction,
};
use sui_types::messages::TransactionEnvelope;
use futures::future::try_join_all;
use test_utils::authority::{
    spawn_test_authorities, test_authority_aggregator, test_authority_configs,
};
use test_utils::transaction::publish_counter_package;
use test_utils::messages::{make_transfer_sui_transaction, make_counter_create_transaction, make_counter_increment_transaction};
use test_utils::objects::{test_gas_objects, generate_gas_objects, generate_gas_object, download_object_from_authorities};
use std::sync::Arc;

async fn setup() -> (
    Vec<SuiNode>,
    AuthorityAggregator<NetworkAuthorityClient>,
    Transaction,
) {
    let mut gas_objects = test_gas_objects();
    let configs = test_authority_configs();
    let handles = spawn_test_authorities(gas_objects.clone(), &configs).await;
    let clients = test_authority_aggregator(&configs);
    let tx = make_transfer_sui_transaction(gas_objects.pop().unwrap(), SuiAddress::default());
    (handles, clients, tx)
}

#[tokio::test]
async fn test_benchmark() {
    let num_transactions = 500;
    
    let mut gas_objects = generate_gas_objects(num_transactions);
    let publish_gas = generate_gas_object();
    let create_counter_gas = generate_gas_object();
    //let publish_gas_ref = publish_gas.compute_object_reference();
    let create_counter_gas_ref = create_counter_gas.compute_object_reference();
    
    gas_objects.push(publish_gas.clone());
    gas_objects.push(create_counter_gas);
    
    let configs = test_authority_configs();
    let _ = spawn_test_authorities(gas_objects.clone(), &configs).await;
    
    let clients = test_authority_aggregator(&configs);
    let quorum_driver_handler = QuorumDriverHandler::new(clients);
    let quorum_driver = quorum_driver_handler.clone_quorum_driver();
    
    // publish package
    let package_ref = publish_counter_package(publish_gas, &configs.validator_set()).await;
    
    // create counter
    let tx = make_counter_create_transaction(create_counter_gas_ref, package_ref);
    let counter_id = if let ExecuteTransactionResponse::EffectsCert(result) = quorum_driver.execute_transaction(ExecuteTransactionRequest {
        transaction: tx,
        request_type: ExecuteTransactionRequestType::WaitForEffectsCert,
    })
    .await
    .unwrap() {
        let (_, effects) = *result;
        effects.effects.created[0].0.clone().0
    } else {
        unreachable!();
    }; 
    
    // remove publish and create counter gas from vec
    gas_objects.pop();
    gas_objects.pop();

    // increment counter for every gas object
    let txs: Vec<_> = gas_objects.into_iter().map(|gas| {
        make_counter_increment_transaction(gas.compute_object_reference(), package_ref, counter_id)
    }).collect();

    // This is the total number of transactions in flight
    let num_workers = 100;
    let tx_per_worker = txs.len() / num_workers;
    let partitioned: Vec<Vec<TransactionEnvelope<_>>> = txs.chunks(tx_per_worker).map(|s| s.into()).collect();
    let mut tasks = Vec::new();
    let num_success = Arc::new(AtomicU64::new(0));
    let num_error = Arc::new(AtomicU64::new(0));
    (0..num_workers).for_each(|i|{
        let p = partitioned[i].clone();
        let qd = quorum_driver.clone();
        let num_error_cloned = num_error.clone();
        let num_success_cloned = num_success.clone();
        let task = tokio::spawn(async move {
            for tx in p.into_iter() {
                let res = qd.execute_transaction(ExecuteTransactionRequest {
                    transaction: tx,
                    request_type: ExecuteTransactionRequestType::WaitForEffectsCert,
                })
                .await;
                if res.is_err() {
                    eprintln!("{}",res.unwrap_err().to_string());
                    num_error_cloned.fetch_add(1, Ordering::SeqCst);
                    continue;
                }
                let _ = if let ExecuteTransactionResponse::EffectsCert(result) = res
                .unwrap() {
                    let (cert, effects) = *result;
                    cert.digest().clone()
                } else {
                    eprintln!("Failed to get effects");
                    num_error_cloned.fetch_add(1, Ordering::SeqCst);
                    continue;
                };
                num_success_cloned.fetch_add(1, Ordering::SeqCst);
            }
        });
        tasks.push(task);
    });
    let _: Vec<_> = try_join_all(tasks)
        .await
        .unwrap()
        .into_iter()
        .collect();
    eprintln!("success = {}, error = {}", num_success.load(Ordering::SeqCst), num_error.load(Ordering::SeqCst));
}

#[tokio::test]
async fn test_execute_transaction_immediate() {
    let (_handles, clients, tx) = setup().await;
    let digest = *tx.digest();

    let mut quorum_driver_handler = QuorumDriverHandler::new(clients);
    let quorum_driver = quorum_driver_handler.clone_quorum_driver();
    let handle = tokio::task::spawn(async move {
        let (cert, effects) = quorum_driver_handler.subscribe().recv().await.unwrap();
        assert_eq!(*cert.digest(), digest);
        assert_eq!(effects.effects.transaction_digest, digest);
    });
    assert!(matches!(
        quorum_driver
            .execute_transaction(ExecuteTransactionRequest {
                transaction: tx,
                request_type: ExecuteTransactionRequestType::ImmediateReturn,
            })
            .await
            .unwrap(),
        ExecuteTransactionResponse::ImmediateReturn
    ));

    handle.await.unwrap();
}

#[tokio::test]
async fn test_execute_transaction_wait_for_cert() {
    let (_handles, clients, tx) = setup().await;
    let digest = *tx.digest();

    let mut quorum_driver_handler = QuorumDriverHandler::new(clients);
    let quorum_driver = quorum_driver_handler.clone_quorum_driver();
    let handle = tokio::task::spawn(async move {
        let (cert, effects) = quorum_driver_handler.subscribe().recv().await.unwrap();
        assert_eq!(*cert.digest(), digest);
        assert_eq!(effects.effects.transaction_digest, digest);
    });
    if let ExecuteTransactionResponse::TxCert(cert) = quorum_driver
        .execute_transaction(ExecuteTransactionRequest {
            transaction: tx,
            request_type: ExecuteTransactionRequestType::WaitForTxCert,
        })
        .await
        .unwrap()
    {
        assert_eq!(*cert.digest(), digest);
    } else {
        unreachable!();
    }

    handle.await.unwrap();
}

#[tokio::test]
async fn test_execute_transaction_wait_for_effects() {
    let (_handles, clients, tx) = setup().await;
    let digest = *tx.digest();

    let mut quorum_driver_handler = QuorumDriverHandler::new(clients);
    let quorum_driver = quorum_driver_handler.clone_quorum_driver();
    let handle = tokio::task::spawn(async move {
        let (cert, effects) = quorum_driver_handler.subscribe().recv().await.unwrap();
        assert_eq!(*cert.digest(), digest);
        assert_eq!(effects.effects.transaction_digest, digest);
    });
    if let ExecuteTransactionResponse::EffectsCert(result) = quorum_driver
        .execute_transaction(ExecuteTransactionRequest {
            transaction: tx,
            request_type: ExecuteTransactionRequestType::WaitForEffectsCert,
        })
        .await
        .unwrap()
    {
        let (cert, effects) = *result;
        assert_eq!(*cert.digest(), digest);
        assert_eq!(effects.effects.transaction_digest, digest);
    } else {
        unreachable!();
    }

    handle.await.unwrap();
}

#[tokio::test]
async fn test_update_validators() {
    let (_handles, mut clients, tx) = setup().await;
    let quorum_driver_handler = QuorumDriverHandler::new(clients.clone());
    let quorum_driver = quorum_driver_handler.clone_quorum_driver();
    let handle = tokio::task::spawn(async move {
        // Wait till the epoch/committee is updated.
        tokio::time::sleep(Duration::from_secs(3)).await;

        let result = quorum_driver
            .execute_transaction(ExecuteTransactionRequest {
                transaction: tx,
                request_type: ExecuteTransactionRequestType::WaitForEffectsCert,
            })
            .await;
        // This now will fail due to epoch mismatch.
        assert!(result.is_err());
    });

    // Create a new authority aggregator with a new epoch number, and update the quorum driver.
    clients.committee.epoch = 10;
    quorum_driver_handler
        .update_validators(clients)
        .await
        .unwrap();

    handle.await.unwrap();
}
