pub mod reg_topic;
pub mod topic_query;

use crate::publish_and_collect;
use chrono::Local;
use discv5::{
    enr::{CombinedKey, EnrBuilder},
    service::RegistrationState,
    Discv5, Discv5Config, Enr,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashSet},
    fmt::Debug,
    net::SocketAddr,
};
use testground::{client::Client, WriteQuery};
use tokio::task;
use tokio_stream::StreamExt;
use tracing::{debug, error, info};

const STATE_COMPLETED_TO_COLLECT_INSTANCE_INFORMATION: &str =
    "state_completed_to_collect_instance_information";
const STATE_COMPLETED_TO_BUILD_TOPOLOGY: &str = "state_completed_to_build_topology";
const STATE_DONE: &str = "state_done";

trait InstanceInfo {
    fn seq(&self) -> u64;
    fn enr(&self) -> &Enr;
}

async fn collect_other_instance_info(
    client: &Client,
    instance_info: &(impl InstanceInfo + Clone + Debug + Serialize + for<'de> Deserialize<'de>),
) -> Result<impl Iterator<Item = impl InstanceInfo>, Box<dyn std::error::Error>> {
    let mut info = publish_and_collect(client, instance_info.clone()).await?;

    if let Some(pos) = info.iter().position(|i| i.seq() == instance_info.seq()) {
        info.remove(pos);
    }

    Ok(info.into_iter())
}
