// Copyright (c) 2022, Mysten Labs, Inc.
// SPDX-License-Identifier: Apache-2.0

import { CookiesConsent } from '../cookies-consent/CookiesConsent';
import ExternalLink from '../external-link/ExternalLink';

import styles from './Footer.module.css';

function Footer() {
    return (
        <div className={styles.footer}>
            <nav className={styles.links}>
                <div>
                    <h6>Read</h6>
                    <ul>
                        <li>
                            <ExternalLink
                                href="https://medium.com/mysten-labs"
                                label="Blog"
                            />
                        </li>
                        <li>
                            <ExternalLink
                                href="https://github.com/MystenLabs/sui/blob/main/doc/paper/sui.pdf"
                                label="Whitepaper"
                            />
                        </li>
                    </ul>
                </div>
                <div>
                    <h6>Build</h6>
                    <ul>
                        <li>
                            <ExternalLink
                                href="https://docs.sui.io/"
                                label="Docs"
                            />
                        </li>
                        <li>
                            <ExternalLink
                                href="https://github.com/MystenLabs"
                                label="GitHub"
                            />
                        </li>
                        <li>
                            <ExternalLink
                                href="https://discord.gg/sui"
                                label="Discord"
                            />
                        </li>
                    </ul>
                </div>
                <div>
                    <h6>Follow</h6>
                    <ul>
                        <li>
                            <ExternalLink
                                href="https://mystenlabs.com/#community"
                                label="Press"
                            />
                        </li>
                        <li>
                            <ExternalLink
                                href="https://twitter.com/mysten_labs"
                                label="Twitter"
                            />
                        </li>
                        <li>
                            <ExternalLink
                                href="https://www.linkedin.com/company/mysten-labs/"
                                label="LinkedIn"
                            />
                        </li>
                    </ul>
                </div>
            </nav>
            <CookiesConsent />
        </div>
    );
}

export default Footer;
/*
                <Link to="/" id="homeBtn">
                    Home
                </Link>
                <ExternalLink href="https://sui.io/" label="Sui" />
                <ExternalLink
                    href="https://mystenlabs.com/"
                    label="Mysten Labs"
                />
                <ExternalLink
                    href="https://docs.sui.io/"
                    label="Developer Hub"
                />
                */
