// Copyright (c) The Diem Core Contributors
// SPDX-License-Identifier: Apache-2.0

use crate::{
    driver::DriverConfiguration, error::Error, notification_handlers::ConsensusSyncRequest,
    storage_synchronizer::StorageSynchronizerInterface, utils, utils::SpeculativeStreamState,
};
use aptos_config::config::ContinuousSyncingMode;
use aptos_infallible::Mutex;
use aptos_types::{
    ledger_info::LedgerInfoWithSignatures,
    transaction::{TransactionListWithProof, TransactionOutputListWithProof, Version},
};
use data_streaming_service::{
    data_notification::{DataNotification, DataPayload, NotificationId},
    data_stream::DataStreamListener,
    streaming_client::{DataStreamingClient, NotificationFeedback, StreamingServiceClient},
};
use std::sync::Arc;
use storage_interface::DbReader;

/// A simple component that manages the continuous syncing of the node
pub struct ContinuousSyncer<StorageSyncer> {
    // The currently active data stream (provided by the data streaming service)
    active_data_stream: Option<DataStreamListener>,

    // The config of the state sync driver
    driver_configuration: DriverConfiguration,

    // The speculative state tracking the active data stream
    speculative_stream_state: Option<SpeculativeStreamState>,

    // The client through which to stream data from the Diem network
    streaming_service_client: StreamingServiceClient,

    // The interface to read from storage
    storage: Arc<dyn DbReader>,

    // The storage synchronizer used to update local storage
    storage_synchronizer: StorageSyncer,
}

impl<StorageSyncer: StorageSynchronizerInterface + Clone> ContinuousSyncer<StorageSyncer> {
    pub fn new(
        driver_configuration: DriverConfiguration,
        streaming_service_client: StreamingServiceClient,
        storage: Arc<dyn DbReader>,
        storage_synchronizer: StorageSyncer,
    ) -> Self {
        Self {
            active_data_stream: None,
            driver_configuration,
            speculative_stream_state: None,
            streaming_service_client,
            storage,
            storage_synchronizer,
        }
    }

    /// Checks if the continuous syncer is able to make progress
    pub async fn drive_progress(
        &mut self,
        consensus_sync_request: Arc<Mutex<Option<ConsensusSyncRequest>>>,
    ) -> Result<(), Error> {
        if self.active_data_stream.is_some() {
            // We have an active data stream. Process any notifications!
            self.process_active_stream_notifications(consensus_sync_request)
                .await
        } else if self.storage_synchronizer.pending_transaction_data() {
            // Wait for any pending transaction data to be processed
            Ok(())
        } else {
            // Fetch a new data stream to start streaming data
            self.initialize_active_data_stream(consensus_sync_request)
                .await
        }
    }

    /// Initializes an active data stream so that we can begin to process notifications
    async fn initialize_active_data_stream(
        &mut self,
        consensus_sync_request: Arc<Mutex<Option<ConsensusSyncRequest>>>,
    ) -> Result<(), Error> {
        // Fetch transactions or outputs starting at highest_synced_version + 1
        let (highest_synced_version, highest_synced_epoch) =
            self.get_highest_synced_version_and_epoch()?;
        let next_version = highest_synced_version
            .checked_add(1)
            .ok_or_else(|| Error::IntegerOverflow("The next version has overflown!".into()))?;

        // Initialize a new active data stream
        let sync_request_target = consensus_sync_request
            .lock()
            .as_ref()
            .map(|sync_request| sync_request.get_sync_target());
        let active_data_stream = match self.driver_configuration.config.continuous_syncing_mode {
            ContinuousSyncingMode::ApplyTransactionOutputs => {
                self.streaming_service_client
                    .continuously_stream_transaction_outputs(
                        next_version,
                        highest_synced_epoch,
                        sync_request_target,
                    )
                    .await?
            }
            ContinuousSyncingMode::ExecuteTransactions => {
                self.streaming_service_client
                    .continuously_stream_transactions(
                        next_version,
                        highest_synced_epoch,
                        false,
                        sync_request_target,
                    )
                    .await?
            }
        };
        self.speculative_stream_state = Some(SpeculativeStreamState::new(
            utils::fetch_latest_epoch_state(self.storage.clone())?,
            None,
            highest_synced_version,
        ));
        self.active_data_stream = Some(active_data_stream);

        Ok(())
    }

    /// Processes any notifications already pending on the active stream
    async fn process_active_stream_notifications(
        &mut self,
        consensus_sync_request: Arc<Mutex<Option<ConsensusSyncRequest>>>,
    ) -> Result<(), Error> {
        loop {
            // Fetch and process any data notifications
            let data_notification =
                utils::get_data_notification(self.active_data_stream.as_mut()).await?;
            match data_notification.data_payload {
                DataPayload::ContinuousTransactionOutputsWithProof(
                    ledger_info_with_sigs,
                    transaction_outputs_with_proof,
                ) => {
                    let payload_start_version =
                        transaction_outputs_with_proof.first_transaction_output_version;
                    self.process_transaction_or_output_payload(
                        consensus_sync_request.clone(),
                        data_notification.notification_id,
                        ledger_info_with_sigs,
                        None,
                        Some(transaction_outputs_with_proof),
                        payload_start_version,
                    )
                    .await?;
                }
                DataPayload::ContinuousTransactionsWithProof(
                    ledger_info_with_sigs,
                    transactions_with_proof,
                ) => {
                    let payload_start_version = transactions_with_proof.first_transaction_version;
                    self.process_transaction_or_output_payload(
                        consensus_sync_request.clone(),
                        data_notification.notification_id,
                        ledger_info_with_sigs,
                        Some(transactions_with_proof),
                        None,
                        payload_start_version,
                    )
                    .await?;
                }
                _ => {
                    return self
                        .handle_end_of_stream_or_invalid_payload(data_notification)
                        .await;
                }
            }
        }
    }

    /// Returns the highest synced version and epoch in storage
    fn get_highest_synced_version_and_epoch(&self) -> Result<(Version, Version), Error> {
        let highest_synced_version = utils::fetch_latest_synced_version(self.storage.clone())?;
        let highest_synced_epoch = utils::fetch_latest_epoch_state(self.storage.clone())?.epoch;

        Ok((highest_synced_version, highest_synced_epoch))
    }

    /// Process a single transaction or transaction output data payload
    async fn process_transaction_or_output_payload(
        &mut self,
        consensus_sync_request: Arc<Mutex<Option<ConsensusSyncRequest>>>,
        notification_id: NotificationId,
        ledger_info_with_signatures: LedgerInfoWithSignatures,
        transaction_list_with_proof: Option<TransactionListWithProof>,
        transaction_outputs_with_proof: Option<TransactionOutputListWithProof>,
        payload_start_version: Option<Version>,
    ) -> Result<(), Error> {
        // Verify the payload starting version
        let payload_start_version = self
            .verify_payload_start_version(notification_id, payload_start_version)
            .await?;

        // Verify the given proof ledger info
        self.verify_proof_ledger_info(
            consensus_sync_request.clone(),
            notification_id,
            &ledger_info_with_signatures,
        )
        .await?;

        // Execute/apply and commit the transactions/outputs
        let num_transactions_or_outputs =
            match self.driver_configuration.config.continuous_syncing_mode {
                ContinuousSyncingMode::ApplyTransactionOutputs => {
                    if let Some(transaction_outputs_with_proof) = transaction_outputs_with_proof {
                        let num_transaction_outputs = transaction_outputs_with_proof
                            .transactions_and_outputs
                            .len();
                        self.storage_synchronizer.apply_transaction_outputs(
                            notification_id,
                            transaction_outputs_with_proof,
                            ledger_info_with_signatures,
                            None,
                        )?;
                        num_transaction_outputs
                    } else {
                        self.terminate_active_stream(
                            notification_id,
                            NotificationFeedback::PayloadTypeIsIncorrect,
                        )
                        .await?;
                        return Err(Error::InvalidPayload(
                            "Did not receive transaction outputs with proof!".into(),
                        ));
                    }
                }
                ContinuousSyncingMode::ExecuteTransactions => {
                    if let Some(transaction_list_with_proof) = transaction_list_with_proof {
                        let num_transactions = transaction_list_with_proof.transactions.len();
                        self.storage_synchronizer.execute_transactions(
                            notification_id,
                            transaction_list_with_proof,
                            ledger_info_with_signatures,
                            None,
                        )?;
                        num_transactions
                    } else {
                        self.terminate_active_stream(
                            notification_id,
                            NotificationFeedback::PayloadTypeIsIncorrect,
                        )
                        .await?;
                        return Err(Error::InvalidPayload(
                            "Did not receive transactions with proof!".into(),
                        ));
                    }
                }
            };
        let synced_version = payload_start_version
            .checked_add(num_transactions_or_outputs as u64)
            .and_then(|version| version.checked_sub(1)) // synced_version = start + num txns/outputs - 1
            .ok_or_else(|| Error::IntegerOverflow("The synced version has overflown!".into()))?;
        self.get_speculative_stream_state()
            .update_synced_version(synced_version);

        Ok(())
    }

    /// Verifies the first payload version matches the version we wish to sync
    async fn verify_payload_start_version(
        &mut self,
        notification_id: NotificationId,
        payload_start_version: Option<Version>,
    ) -> Result<Version, Error> {
        // Compare the payload start version with the expected version
        let expected_version = self
            .get_speculative_stream_state()
            .expected_next_version()?;
        if let Some(payload_start_version) = payload_start_version {
            if payload_start_version != expected_version {
                self.terminate_active_stream(
                    notification_id,
                    NotificationFeedback::InvalidPayloadData,
                )
                .await?;
                Err(Error::VerificationError(format!(
                    "The payload start version does not match the expected version! Start: {:?}, expected: {:?}",
                    payload_start_version, expected_version
                )))
            } else {
                Ok(payload_start_version)
            }
        } else {
            self.terminate_active_stream(notification_id, NotificationFeedback::EmptyPayloadData)
                .await?;
            Err(Error::VerificationError(
                "The playload starting version is missing!".into(),
            ))
        }
    }

    /// Verifies the given ledger info to be used as a transaction or transaction
    /// output chunk proof. If verification fails, the active stream is terminated.
    async fn verify_proof_ledger_info(
        &mut self,
        consensus_sync_request: Arc<Mutex<Option<ConsensusSyncRequest>>>,
        notification_id: NotificationId,
        ledger_info_with_signatures: &LedgerInfoWithSignatures,
    ) -> Result<(), Error> {
        // If we're syncing to a specific target, verify the ledger info isn't too high
        let sync_request_target = consensus_sync_request
            .lock()
            .as_ref()
            .map(|sync_request| sync_request.get_sync_target());
        if let Some(sync_request_target) = sync_request_target {
            let sync_request_version = sync_request_target.ledger_info().version();
            let proof_version = ledger_info_with_signatures.ledger_info().version();
            if sync_request_version < proof_version {
                self.terminate_active_stream(
                    notification_id,
                    NotificationFeedback::PayloadProofFailed,
                )
                .await?;
                return Err(Error::VerificationError(format!(
                    "Proof version is higher than the sync target. Proof version: {:?}, sync version: {:?}.",
                    proof_version, sync_request_version
                )));
            }
        }

        // Verify the ledger info state and signatures
        if let Err(error) = self
            .get_speculative_stream_state()
            .verify_ledger_info_with_signatures(ledger_info_with_signatures)
        {
            self.terminate_active_stream(notification_id, NotificationFeedback::PayloadProofFailed)
                .await?;
            Err(error)
        } else {
            Ok(())
        }
    }

    /// Handles the end of stream notification or an invalid payload by
    /// terminating the stream appropriately.
    async fn handle_end_of_stream_or_invalid_payload(
        &mut self,
        data_notification: DataNotification,
    ) -> Result<(), Error> {
        self.reset_active_stream();

        utils::handle_end_of_stream_or_invalid_payload(
            &mut self.streaming_service_client,
            data_notification,
        )
        .await
    }

    /// Terminates the currently active stream with the provided feedback
    pub async fn terminate_active_stream(
        &mut self,
        notification_id: NotificationId,
        notification_feedback: NotificationFeedback,
    ) -> Result<(), Error> {
        self.reset_active_stream();

        utils::terminate_stream_with_feedback(
            &mut self.streaming_service_client,
            notification_id,
            notification_feedback,
        )
        .await
    }

    /// Returns the speculative stream state. Assumes that the state exists.
    fn get_speculative_stream_state(&mut self) -> &mut SpeculativeStreamState {
        self.speculative_stream_state
            .as_mut()
            .expect("Speculative stream state does not exist!")
    }

    /// Resets the currently active data stream and speculative state
    fn reset_active_stream(&mut self) {
        self.speculative_stream_state = None;
        self.active_data_stream = None;
    }
}
