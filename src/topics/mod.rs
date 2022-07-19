use crate::publish_and_collect;
use chrono::Local;
use discv5::{
    enr::{CombinedKey, EnrBuilder},
    Discv5, Discv5Config, Enr,
};
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use testground::{client::Client, WriteQuery};
use tokio::{
    task,
    time::{sleep, Duration},
};
use tracing::{debug, error, info};

const STATE_COMPLETED_TO_COLLECT_INSTANCE_INFORMATION: &str =
    "state_completed_to_collect_instance_information";
const STATE_COMPLETED_TO_BUILD_TOPOLOGY: &str = "state_completed_to_build_topology";
const STATE_COMPLETED_TO_REGISTER_TOPIC: &str = "state_completed_to_register_topic";

#[derive(Clone, Debug, Serialize, Deserialize)]
struct InstanceInfo {
    // The sequence number of this test instance within the test.
    seq: u64,
    enr: Enr,
    is_registrant: bool,
}

impl InstanceInfo {
    async fn new(client: &Client, enr: Enr) -> Result<Self, Box<dyn std::error::Error>> {
        let seq = client.global_seq();

        // NOTE: For now, #1 is the registrant node.
        let is_registrant = seq == 1;

        Ok(InstanceInfo {
            seq,
            enr,
            is_registrant,
        })
    }
}

pub(super) async fn reg_topic(client: Client) -> Result<(), Box<dyn std::error::Error>> {
    let run_parameters = client.run_parameters();
    // ////////////////////////
    // Construct a local Enr
    // ////////////////////////
    let enr_key = CombinedKey::generate_secp256k1();
    let enr = EnrBuilder::new("v4")
        .ip(run_parameters
            .data_network_ip()?
            .expect("IP address for the data network"))
        .udp4(9000)
        .build(&enr_key)
        .expect("Construct an Enr");

    info!("ENR: {:?}", enr);
    info!("NodeId: {}", enr.node_id());

    // //////////////////////////////////////////////////////////////
    // Start Discovery v5 server
    // //////////////////////////////////////////////////////////////
    let mut discv5 = Discv5::new(enr, enr_key, Discv5Config::default())?;
    discv5
        .start("0.0.0.0:9000".parse::<SocketAddr>()?)
        .await
        .expect("Start Discovery v5 server");

    // Observe Discv5 events.
    let mut event_stream = discv5.event_stream().await.expect("Discv5Event");
    task::spawn(async move {
        while let Some(event) = event_stream.recv().await {
            info!("Discv5Event: {:?}", event);
        }
    });

    // //////////////////////////////////////////////////////////////
    // Collect information of all participants in the test case
    // //////////////////////////////////////////////////////////////
    let instance_info = InstanceInfo::new(&client, discv5.local_enr()).await?;
    debug!("instance_info: {:?}", instance_info);

    let other_instances = collect_other_instance_info(&client, &instance_info).await?;
    debug!("other_instances: {:?}", other_instances);

    client
        .signal_and_wait(
            STATE_COMPLETED_TO_COLLECT_INSTANCE_INFORMATION,
            run_parameters.test_instance_count,
        )
        .await?;

    for i in other_instances.iter() {
        discv5.add_enr(i.enr.clone())?;
    }

    client
        .signal_and_wait(
            STATE_COMPLETED_TO_BUILD_TOPOLOGY,
            run_parameters.test_instance_count,
        )
        .await?;

    // //////////////////////////////////////////////////////////////
    // Register topic
    // //////////////////////////////////////////////////////////////
    let mut failed = false;

    if instance_info.is_registrant {
        let _ = discv5.register_topic("lighthouse").await.map_err(|e| {
            failed = true;
            error!("Failed to register topic. Error: {}", e);
        });

        for (distance, bucket) in discv5
            .reg_attempts("lighthouse")
            .await
            .map_err(|e| error!("Failed to get registration attempts. Error {}", e))
            .unwrap()
        {
            let mut table_entries_id_topic = discv5
                .table_entries_id_topic("lighthouse")
                .await
                .map_err(|e| error!("Failed to get table entries' ids for topic. Error {}", e))
                .unwrap()
                .into_iter();

            info!("At distance {}:", distance);
            info!("{} registration attempts", bucket.reg_attempts.len());
            let (kbucket_index, peer_ids) = table_entries_id_topic.next().unwrap();
            info!(
                "{} peers in topic's kbuckets at distance {}",
                peer_ids.len(),
                kbucket_index
            );
        }
    }

    if !instance_info.is_registrant {
        // Sleep for the duration of a registration window plus 15 seconds to sync all nodes.
        sleep(Duration::from_secs(25)).await;
        let ads = discv5
            .ads("lighthouse")
            .await
            .map_err(|e| error!("Failed to register topic. Error: {}", e))
            .unwrap();
        if ads.is_empty() {
            failed = true;
        }
    }

    // //////////////////////////////////////////////////////////////
    // Record metrics
    // //////////////////////////////////////////////////////////////
    let metrics = discv5.metrics();
    let write_query = WriteQuery::new(
        Local::now().into(),
        format!(
            "discv5-testground_{}_{}",
            run_parameters.test_case, run_parameters.test_run
        ),
    )
    .add_field("topics_to_publish", metrics.topics_to_publish as u64)
    .add_field("hosted_ads", metrics.hosted_ads as u64)
    .add_field("active_regtopic_req", metrics.active_regtopic_req as u64);

    client.record_metric(write_query).await?;

    client
        .signal_and_wait(
            STATE_COMPLETED_TO_REGISTER_TOPIC,
            run_parameters.test_instance_count,
        )
        .await?;

    // //////////////////////////////////////////////////////////////
    // Record result of this test
    // //////////////////////////////////////////////////////////////
    if failed {
        info!(
            "Failed. {} entries in local routing table.",
            discv5.table_entries_id().len()
        );
        client
            .record_failure("Failures have happened, please check error logs for details.")
            .await?;
    } else {
        client.record_success().await?;
    }

    Ok(())
}

async fn collect_other_instance_info(
    client: &Client,
    instance_info: &InstanceInfo,
) -> Result<Vec<InstanceInfo>, Box<dyn std::error::Error>> {
    let mut info = publish_and_collect(client, instance_info.clone()).await?;

    if let Some(pos) = info.iter().position(|i| i.seq == instance_info.seq) {
        info.remove(pos);
    }

    Ok(info)
}
