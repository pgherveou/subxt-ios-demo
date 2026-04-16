use std::sync::Arc;
use std::time::{Duration, Instant};
use subxt::lightclient::LightClient;
use subxt::{OnlineClient, PolkadotConfig};
use tokio::sync::Notify;

const POLKADOT_SPEC: &str = include_str!("../polkadot.json");
const ASSET_HUB_SPEC: &str = include_str!("../asset_hub.json");

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

/// Token passed to `subscribe` to cancel the subscription from Swift.
/// Call `cancel()` or let it be deallocated to stop the block stream.
#[derive(uniffi::Object)]
pub struct CancellationToken {
    notify: Arc<Notify>,
}

impl Drop for CancellationToken {
    fn drop(&mut self) {
        eprintln!("[subxt] CancellationToken dropped, signalling cancel");
        self.notify.notify_one();
    }
}

#[uniffi::export]
impl CancellationToken {
    #[uniffi::constructor]
    pub fn new() -> Arc<Self> {
        eprintln!("[subxt] CancellationToken created");
        Arc::new(Self {
            notify: Arc::new(Notify::new()),
        })
    }

    pub fn cancel(&self) {
        eprintln!("[subxt] CancellationToken.cancel() called");
        self.notify.notify_one();
    }
}

/// Block streaming loop shared between RPC and light client modes.
/// Uses a macro because `Blocks<T>` is not publicly exported from subxt.
macro_rules! stream_blocks {
    ($blocks:expr, $cancel:expr, $listener:expr, $t0:expr) => {{
        let elapsed = move || format!("{:.1}s", $t0.elapsed().as_secs_f64());
        let mut count = 0u64;

        loop {
            tokio::select! {
                block_result = $blocks.next() => {
                    match block_result {
                        Some(Ok(block)) => {
                            count += 1;
                            let info = BlockInfo {
                                number: block.number(),
                                hash: format!("{}", block.hash()),
                            };
                            $listener.on_new_block(info)?;
                            if count == 1 {
                                $listener.on_status_change(format!(
                                    "First block at {} - #{}", elapsed(), block.number()
                                ))?;
                            }
                        }
                        Some(Err(e)) => {
                            $listener.on_error(format!("Block stream error: {e}"))?;
                        }
                        None => {
                            eprintln!("[subxt] block stream ended after {count} blocks");
                            $listener.on_status_change("Block stream ended".into())?;
                            break;
                        }
                    }
                }
                _ = $cancel.notified() => {
                    eprintln!("[subxt] cancelled after {count} blocks ({elapsed})", elapsed = elapsed());
                    $listener.on_status_change("Subscription cancelled".into())?;
                    break;
                }
            }
        }

        eprintln!("[subxt] stream_blocks exiting, dropping block stream");
        Ok(())
    }};
}

/// Subscribe to Asset Hub blocks. Runs until cancelled or the stream ends.
#[uniffi::export(async_runtime = "tokio")]
pub async fn subscribe(
    mode: ConnectionMode,
    cancel: Arc<CancellationToken>,
    listener: Arc<dyn BlockListener>,
) -> Result<(), SubxtError> {
    eprintln!("[subxt] subscribe() called");
    let notify = cancel.notify.clone();
    let result = match mode {
        ConnectionMode::Rpc { url } => run_rpc(&url, notify, listener).await,
        ConnectionMode::LightClient => run_light_client(notify, listener).await,
    };
    eprintln!("[subxt] subscribe() returning: {result:?}");
    result
}

async fn run_rpc(
    url: &str,
    cancel: Arc<Notify>,
    listener: Arc<dyn BlockListener>,
) -> Result<(), SubxtError> {
    let t0 = Instant::now();
    let elapsed = || format!("{:.1}s", t0.elapsed().as_secs_f64());

    listener.on_status_change(format!("Connecting to RPC... ({})", elapsed()))?;

    let api = OnlineClient::<PolkadotConfig>::from_url(url)
        .await
        .map_err(|e| SubxtError::Subscription { msg: e.to_string() })?;

    listener.on_status_change(format!("Connected ({}), subscribing...", elapsed()))?;

    let mut blocks = api
        .stream_all_blocks()
        .await
        .map_err(|e| SubxtError::Subscription { msg: e.to_string() })?;

    stream_blocks!(blocks, cancel, listener, t0)
}

async fn run_light_client(
    cancel: Arc<Notify>,
    listener: Arc<dyn BlockListener>,
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

    // stream_all_blocks() waits for the first finalized block, which can take
    // a while as the light client syncs. Report progress while waiting.
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
                eprintln!("[subxt] cancelled during light client sync");
                listener.on_status_change("Subscription cancelled".into())?;
                return Ok(());
            }
        }
    };

    listener.on_status_change(format!("Receiving blocks... ({})", elapsed()))?;

    stream_blocks!(blocks, cancel, listener, t0)
}

uniffi::setup_scaffolding!();
