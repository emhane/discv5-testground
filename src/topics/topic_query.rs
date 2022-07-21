use super::InstanceInfo as IInfo;
use super::*;

const SEQ_REGISTRANT: u64 = 1;
const SEQ_QUERENT: u64 = 2;

#[derive(Clone, Debug, Serialize, Deserialize)]
struct InstanceInfo {
    // The sequence number of this test instance within the test.
    seq: u64,
    enr: Enr,
    is_registrant: bool,
    is_querent: bool,
}

impl IInfo for InstanceInfo {
    fn seq(&self) -> u64 {
        self.seq
    }

    fn enr(&self) -> &Enr {
        &self.enr
    }
}

impl InstanceInfo {
    async fn new(client: &Client, enr: Enr) -> Result<Self, Box<dyn std::error::Error>> {
        let seq = client.global_seq();

        // NOTE: For now, #1 is the registrant node.
        let is_registrant = seq == SEQ_REGISTRANT;
        // NOTE: For now, #2 is the registrant node.
        let is_querent = seq == SEQ_QUERENT;

        Ok(InstanceInfo {
            seq,
            enr,
            is_registrant,
            is_querent,
        })
    }
}

pub async fn topic_query(client: Client) -> Result<(), Box<dyn std::error::Error>> {
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

    client
        .signal_and_wait(
            STATE_COMPLETED_TO_COLLECT_INSTANCE_INFORMATION,
            run_parameters.test_instance_count,
        )
        .await?;

    // //////////////////////////////////////////////////////////////
    // Topology
    // //////////////////////////////////////////////////////////////

    if instance_info.is_registrant || instance_info.is_querent {
        for i in other_instances {
            if instance_info.is_registrant && !i.seq() == SEQ_QUERENT {
                if let Err(e) = discv5.add_enr(i.enr().clone()) {
                    error!("Failed to insert enr with node id {} in registrant's local routing table. Ignoring instance. Error {}", i.enr().node_id(), e);
                }
            } else if instance_info.is_querent && i.seq() == SEQ_REGISTRANT {
                if let Err(e) = discv5.add_enr(i.enr().clone()) {
                    error!("Failed to insert enr with node id {} in querent's local routing table. Ignoring instance. Error {}", i.enr().node_id(), e);
                }
            }
        }
    }

    client
        .signal_and_wait(
            STATE_COMPLETED_TO_BUILD_TOPOLOGY,
            run_parameters.test_instance_count,
        )
        .await?;

    // //////////////////////////////////////////////////////////////
    // Topic Query
    // //////////////////////////////////////////////////////////////

    Ok(())
}
