use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};
use subxt::lightclient::LightClient;
use subxt::{OnlineClient, PolkadotConfig};
use tokio::runtime::Runtime;
use tokio::sync::Notify;

const POLKADOT_SPEC: &str = include_str!("../polkadot.json");
const ASSET_HUB_SPEC: &str = include_str!("../asset_hub.json");

fn runtime() -> &'static Runtime {
    static RT: OnceLock<Runtime> = OnceLock::new();
    RT.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap()
    })
}

#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum SubxtError {
    #[error("Light client error: {msg}")]
    LightClient { msg: String },
    #[error("Subscription error: {msg}")]
    Subscription { msg: String },
    #[error("Callback error: {msg}")]
    Callback { msg: String },
}

impl From<uniffi::UnexpectedUniFFICallbackError> for SubxtError {
    fn from(e: uniffi::UnexpectedUniFFICallbackError) -> Self {
        SubxtError::Callback { msg: e.reason }
    }
}

#[derive(uniffi::Enum)]
pub enum ConnectionMode {
    Rpc { url: String },
    LightClient,
}

#[derive(uniffi::Record)]
pub struct BlockInfo {
    pub number: u64,
    pub hash: String,
}

#[uniffi::export(with_foreign)]
pub trait BlockListener: Send + Sync {
    fn on_status_change(&self, status: String) -> Result<(), SubxtError>;
    fn on_new_block(&self, block: BlockInfo) -> Result<(), SubxtError>;
    fn on_error(&self, error: String) -> Result<(), SubxtError>;
}

#[derive(uniffi::Object)]
pub struct SubscriptionHandle {
    cancel: Arc<Notify>,
}

impl Drop for SubscriptionHandle {
    fn drop(&mut self) {
        self.cancel.notify_one();
    }
}

#[uniffi::export]
impl SubscriptionHandle {
    pub fn cancel(&self) {
        self.cancel.notify_one();
    }
}

#[uniffi::export]
pub fn subscribe(
    mode: ConnectionMode,
    listener: Arc<dyn BlockListener>,
) -> Arc<SubscriptionHandle> {
    let cancel = Arc::new(Notify::new());
    let handle = Arc::new(SubscriptionHandle {
        cancel: cancel.clone(),
    });

    runtime().spawn(async move {
        let result = match mode {
            ConnectionMode::Rpc { url } => run_rpc(&url, listener.clone(), cancel).await,
            ConnectionMode::LightClient => run_light_client(listener.clone(), cancel).await,
        };
        if let Err(e) = result {
            let _ = listener.on_error(e.to_string());
        }
    });

    handle
}

async fn run_rpc(
    url: &str,
    listener: Arc<dyn BlockListener>,
    cancel: Arc<Notify>,
) -> Result<(), SubxtError> {
    let t0 = Instant::now();
    let elapsed = || format!("{:.1}s", t0.elapsed().as_secs_f64());

    listener.on_status_change(format!("Connecting to RPC... ({})", elapsed()))?;

    let api = OnlineClient::<PolkadotConfig>::from_url(url)
        .await
        .map_err(|e| SubxtError::Subscription { msg: e.to_string() })?;

    listener.on_status_change(format!("Connected ({}), subscribing...", elapsed()))?;

    stream_blocks(api, listener, cancel, t0).await
}

async fn run_light_client(
    listener: Arc<dyn BlockListener>,
    cancel: Arc<Notify>,
) -> Result<(), SubxtError> {
    let t0 = Instant::now();
    let elapsed = || format!("{:.1}s", t0.elapsed().as_secs_f64());

    listener.on_status_change(format!("Starting light client... ({})", elapsed()))?;

    let (lc, _relay_rpc) = LightClient::relay_chain(POLKADOT_SPEC)
        .map_err(|e| SubxtError::LightClient { msg: e.to_string() })?;

    listener.on_status_change(format!("Adding Asset Hub... ({})", elapsed()))?;

    let asset_hub_rpc = lc
        .parachain(ASSET_HUB_SPEC)
        .map_err(|e| SubxtError::LightClient { msg: e.to_string() })?;

    listener.on_status_change(format!("Connecting... ({})", elapsed()))?;

    let api = OnlineClient::<PolkadotConfig>::from_rpc_client(asset_hub_rpc)
        .await
        .map_err(|e| SubxtError::LightClient { msg: e.to_string() })?;

    listener.on_status_change(format!("Syncing parachain... ({})", elapsed()))?;

    // stream_all_blocks() internally waits for the first finalized block,
    // which can take a while as the light client syncs. Report progress
    // while waiting.
    let blocks_fut = api.stream_all_blocks();
    tokio::pin!(blocks_fut);

    let mut blocks = loop {
        let tick = tokio::time::sleep(Duration::from_secs(3));
        tokio::select! {
            result = &mut blocks_fut => {
                break result.map_err(|e| SubxtError::Subscription { msg: e.to_string() })?;
            }
            _ = tick => {
                listener.on_status_change(format!("Syncing parachain... ({})", elapsed()))?;
            }
            _ = cancel.notified() => {
                listener.on_status_change("Subscription cancelled".into())?;
                return Ok(());
            }
        }
    };

    listener.on_status_change(format!("Receiving blocks... ({})", elapsed()))?;

    let mut first = true;
    loop {
        tokio::select! {
            block_result = blocks.next() => {
                match block_result {
                    Some(Ok(block)) => {
                        let info = BlockInfo {
                            number: block.number(),
                            hash: format!("{}", block.hash()),
                        };
                        listener.on_new_block(info)?;
                        if first {
                            first = false;
                            listener.on_status_change(format!(
                                "First block at {} - #{}", elapsed(), block.number()
                            ))?;
                        }
                    }
                    Some(Err(e)) => {
                        listener.on_error(format!("Block stream error: {e}"))?;
                    }
                    None => {
                        listener.on_status_change("Block stream ended".into())?;
                        break;
                    }
                }
            }
            _ = cancel.notified() => {
                listener.on_status_change("Subscription cancelled".into())?;
                break;
            }
        }
    }

    Ok(())
}

async fn stream_blocks(
    api: OnlineClient<PolkadotConfig>,
    listener: Arc<dyn BlockListener>,
    cancel: Arc<Notify>,
    t0: Instant,
) -> Result<(), SubxtError> {
    let elapsed = move || format!("{:.1}s", t0.elapsed().as_secs_f64());

    let mut blocks = api
        .stream_all_blocks()
        .await
        .map_err(|e| SubxtError::Subscription { msg: e.to_string() })?;

    listener.on_status_change(format!("Syncing... ({})", elapsed()))?;

    let mut first = true;
    loop {
        tokio::select! {
            block_result = blocks.next() => {
                match block_result {
                    Some(Ok(block)) => {
                        let info = BlockInfo {
                            number: block.number(),
                            hash: format!("{}", block.hash()),
                        };
                        listener.on_new_block(info)?;
                        if first {
                            first = false;
                            listener.on_status_change(format!(
                                "First block at {} - #{}", elapsed(), block.number()
                            ))?;
                        }
                    }
                    Some(Err(e)) => {
                        listener.on_error(format!("Block stream error: {e}"))?;
                    }
                    None => {
                        listener.on_status_change("Block stream ended".into())?;
                        break;
                    }
                }
            }
            _ = cancel.notified() => {
                listener.on_status_change("Subscription cancelled".into())?;
                break;
            }
        }
    }

    Ok(())
}

uniffi::setup_scaffolding!();
