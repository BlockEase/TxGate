//! Desktop shell: owns the engine and the HTTP server, bridges engine events to
//! the web UI, and provides the tray icon and system notifications.

use std::path::PathBuf;
use std::sync::Arc;

use serde::Serialize;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, RunEvent, State, WindowEvent};
use tauri_plugin_notification::NotificationExt;
use tokio::sync::Mutex;
use txgate_core::config::{Config, NetworkConfig};
use txgate_core::engine::{Engine, Event, ServerStatus};
use txgate_core::jsonrpc::SEND_METHODS;
use txgate_core::providers::{Provider, KNOWN_CHAINS};
use txgate_core::server::{self, ServerHandle};
use txgate_core::simulate::SimulationOutcome;
use txgate_core::decode::Severity;
use txgate_core::record::{Decision, RequestRecord};
use txgate_core::safe_state::SafeTxReport;
use txgate_core::safe_watch::{SafeAlert, WatchStatus};
use txgate_core::store::{AddressNote, HistoryQuery, Store};
use txgate_core::upstream::HttpUpstream;

/// Name of the single event channel the UI listens on.
const EVENT_CHANNEL: &str = "txgate://event";

struct AppState {
    engine: Engine,
    server: Mutex<Option<ServerHandle>>,
    config_path: PathBuf,
    db_path: PathBuf,
    /// Problem found while starting up, shown once by the UI.
    startup_warning: Option<String>,
    /// When the user was last warned that quitting discards held transactions.
    exit_warned_ms: std::sync::atomic::AtomicU64,
}

type CmdResult<T> = Result<T, String>;

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

#[derive(Serialize)]
struct ProviderInfo {
    id: Provider,
    name: &'static str,
    chains: Vec<u64>,
}

#[derive(Serialize)]
struct KnownChain {
    chain_id: u64,
    name: &'static str,
}

#[derive(Serialize)]
struct Snapshot {
    config: Config,
    recent: Vec<RequestRecord>,
    pending: Vec<RequestRecord>,
    server: ServerStatus,
    intercepted_methods: &'static [&'static str],
    providers: Vec<ProviderInfo>,
    known_chains: Vec<KnownChain>,
    /// The running TxGate version.
    version: &'static str,
    /// Native currency symbols that are not ETH, by chain id.
    native_currencies: &'static [(u64, &'static str)],
    /// Block explorer base URLs, by chain id.
    explorers: &'static [(u64, &'static str)],
    address_notes: Vec<AddressNote>,
    config_path: String,
    db_path: String,
    lan_addresses: Vec<String>,
    startup_warning: Option<String>,
    safe_watch: Vec<WatchStatus>,
    safe_alerts: Vec<SafeAlert>,
}

/// Best-effort list of this machine's LAN IPv4 addresses, shown when
/// listening on 0.0.0.0 so the user knows what to type into other devices.
fn lan_addresses() -> Vec<String> {
    // Connecting a UDP socket sends nothing but reveals the outbound interface address.
    std::net::UdpSocket::bind("0.0.0.0:0")
        .and_then(|s| {
            s.connect("192.0.2.1:9")?;
            s.local_addr()
        })
        .map(|a| vec![a.ip().to_string()])
        .unwrap_or_default()
}

#[tauri::command]
fn get_snapshot(state: State<'_, AppState>) -> Snapshot {
    let engine = &state.engine;
    Snapshot {
        config: engine.config(),
        recent: engine.recent(),
        pending: engine.pending(),
        server: engine.server_status(),
        intercepted_methods: SEND_METHODS,
        providers: Provider::ALL
            .iter()
            .map(|p| ProviderInfo { id: *p, name: p.display_name(), chains: p.supported_chains() })
            .collect(),
        known_chains: KNOWN_CHAINS.iter().map(|(id, name)| KnownChain { chain_id: *id, name }).collect(),
        native_currencies: txgate_core::providers::NATIVE_CURRENCIES,
        explorers: txgate_core::providers::EXPLORERS,
        address_notes: engine.address_notes().unwrap_or_default(),
        version: env!("CARGO_PKG_VERSION"),
        config_path: state.config_path.display().to_string(),
        db_path: state.db_path.display().to_string(),
        lan_addresses: lan_addresses(),
        startup_warning: state.startup_warning.clone(),
        safe_watch: engine.safe_watch(),
        safe_alerts: engine.safe_alerts(200).unwrap_or_default(),
    }
}

/// Checks a SafeTx before the user signs it (typed data, service JSON or fields).
#[tauri::command]
async fn safe_review(state: State<'_, AppState>, chain_id: u64, safe: String, input: serde_json::Value) -> CmdResult<SafeTxReport> {
    let safe = safe.trim().parse().map_err(|_| "Enter a valid Safe address".to_string())?;
    state.engine.review_safe_tx(chain_id, safe, input).await
}

/// Queued transactions of any Safe, verified locally.
#[tauri::command]
async fn safe_queue(state: State<'_, AppState>, chain_id: u64, safe: String) -> CmdResult<Vec<SafeTxReport>> {
    let safe = safe.trim().parse().map_err(|_| "Enter a valid Safe address".to_string())?;
    state.engine.safe_queue(chain_id, safe).await
}

/// Checks the watched Safes now.
#[tauri::command]
async fn safe_poll(state: State<'_, AppState>) -> CmdResult<Vec<WatchStatus>> {
    Ok(state.engine.poll_safes().await)
}

/// Sets the user's note on an address for one network; an empty note removes it.
#[tauri::command]
fn set_address_note(state: State<'_, AppState>, chain_id: u64, address: String, note: String) -> CmdResult<Vec<AddressNote>> {
    let address = address.trim().parse().map_err(|_| "Not a valid address".to_string())?;
    state.engine.set_address_note(chain_id, address, &note).map_err(err)
}

#[tauri::command]
fn safe_alerts_read(state: State<'_, AppState>) -> CmdResult<Vec<SafeAlert>> {
    state.engine.mark_safe_alerts_read().map_err(err)?;
    state.engine.safe_alerts(200).map_err(err)
}

/// Polls the watched Safes forever, at the configured interval.
fn watch_safes(engine: Engine) {
    tauri::async_runtime::spawn(async move {
        loop {
            if !engine.config().safe.watched.is_empty() {
                engine.poll_safes().await;
            }
            let secs = engine.config().safe.poll_secs.max(15);
            tokio::time::sleep(std::time::Duration::from_secs(secs)).await;
        }
    });
}

#[tauri::command]
async fn decide(state: State<'_, AppState>, id: u64, approve: bool) -> CmdResult<RequestRecord> {
    let decision = if approve { Decision::Approved } else { Decision::Rejected };
    state.engine.decide(id, decision).await.map_err(err)
}

#[tauri::command]
async fn simulate(state: State<'_, AppState>, id: u64) -> CmdResult<SimulationOutcome> {
    state.engine.simulate(id).await.map_err(err)
}

#[tauri::command]
fn query_history(state: State<'_, AppState>, query: HistoryQuery) -> CmdResult<Vec<RequestRecord>> {
    state.engine.history(&query).map_err(err)
}

/// Deletes the request history, keeping transactions that still wait for a decision.
#[tauri::command]
fn clear_history(state: State<'_, AppState>) -> CmdResult<usize> {
    state.engine.clear_history().map_err(err)
}

/// Saves the whole configuration. Restarts the listener when the address changed.
#[tauri::command]
async fn save_config(state: State<'_, AppState>, config: Config) -> CmdResult<ServerStatus> {
    let listen_changed = state.engine.config().listen != config.listen;
    state.engine.set_config(config).map_err(err)?;
    if listen_changed {
        restart_server(&state).await;
    }
    Ok(state.engine.server_status())
}

/// Verifies the network's upstreams report the right chain id, then adds or
/// replaces it (`original_chain_id` identifies the entry being edited).
#[tauri::command]
async fn save_network(state: State<'_, AppState>, network: NetworkConfig, original_chain_id: Option<u64>) -> CmdResult<Config> {
    state.engine.verify_network(&network).await?;
    let replace = original_chain_id.unwrap_or(network.chain_id);
    state
        .engine
        .update_config(|c| match c.networks.iter_mut().find(|n| n.chain_id == replace) {
            Some(slot) => *slot = network,
            None => c.networks.push(network),
        })
        .map_err(err)
}

#[tauri::command]
fn delete_network(state: State<'_, AppState>, chain_id: u64) -> CmdResult<Config> {
    state.engine.update_config(|c| c.networks.retain(|n| n.chain_id != chain_id)).map_err(err)
}

#[tauri::command]
async fn restart_listener(state: State<'_, AppState>) -> CmdResult<ServerStatus> {
    restart_server(&state).await;
    Ok(state.engine.server_status())
}

async fn restart_server(state: &AppState) {
    let mut slot = state.server.lock().await;
    if let Some(handle) = slot.take() {
        handle.stop().await;
    }
    let listen = state.engine.config().listen;
    match server::start(state.engine.clone(), &listen).await {
        Ok(handle) => *slot = Some(handle),
        Err(e) => tracing::error!("failed to listen on {}:{}: {e}", listen.host(), listen.port),
    }
}

/// Loads the configuration. An unreadable file is moved aside rather than
/// silently overwritten by the next save, so the user's keys are not lost.
fn load_config(path: &std::path::Path) -> (Config, Option<String>) {
    match Config::load(path) {
        Ok(c) => (c, None),
        Err(e) => {
            tracing::error!("{e}; starting with defaults");
            let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
            let backup = path.with_extension(format!("broken-{stamp}.toml"));
            let warning = match std::fs::rename(path, &backup) {
                Ok(()) => format!("{e}. TxGate started with default settings; your old file was kept as {}.", backup.display()),
                Err(_) => format!("{e}. TxGate started with default settings; saving settings will replace the file."),
            };
            (Config::default(), Some(warning))
        }
    }
}

/// Opens the request log. A database that cannot be opened is moved aside and
/// a fresh one started; as a last resort the log lives in memory.
fn open_store(path: &std::path::Path) -> (Store, Option<String>) {
    let e = match Store::open(path) {
        Ok(s) => return (s, None),
        Err(e) => e,
    };
    tracing::error!("cannot open {}: {e}", path.display());
    let backup = path.with_extension("broken.sqlite");
    if std::fs::rename(path, &backup).is_ok() {
        if let Ok(s) = Store::open(path) {
            return (s, Some(format!("The request log could not be opened ({e}). It was kept as {} and a new log was started.", backup.display())));
        }
    }
    let s = Store::open_in_memory().expect("in-memory SQLite always opens");
    (s, Some(format!("The request log could not be opened ({e}). This session is not being saved.")))
}

/// Quitting while transactions are held loses them, so the first attempt is
/// refused with a warning; a second attempt within 10 seconds goes through.
fn exit_needs_confirmation(held: usize, last_warned_ms: u64, now_ms: u64) -> bool {
    held > 0 && now_ms.saturating_sub(last_warned_ms) > 10_000
}

fn show_main_window(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
    }
}

/// Language the UI is showing (it resolves "follow the system"); used for
/// notifications and the tray menu.
static UI_LANG: std::sync::RwLock<String> = std::sync::RwLock::new(String::new());

fn tr(key: &str, params: &[(&str, String)]) -> String {
    let lang = UI_LANG.read().map(|l| l.clone()).unwrap_or_default();
    txgate_core::text::tr(&lang, key, params)
}

/// Tray menu entries, relabelled when the language changes.
struct TrayItems {
    show: MenuItem<tauri::Wry>,
    quit: MenuItem<tauri::Wry>,
}

/// Asks GitHub for the latest TxGate release (only when the user clicks).
#[tauri::command]
async fn check_update() -> CmdResult<txgate_core::update::UpdateCheck> {
    txgate_core::update::check_for_update(&HttpUpstream::new(), env!("CARGO_PKG_VERSION")).await
}

/// Called by the UI on start-up and whenever the language changes.
#[tauri::command]
fn set_ui_language(app: AppHandle, language: String) {
    if let Ok(mut l) = UI_LANG.write() {
        *l = language;
    }
    if let Some(items) = app.try_state::<TrayItems>() {
        let _ = items.show.set_text(tr("tray.show", &[]));
        let _ = items.quit.set_text(tr("tray.quit", &[]));
    }
}

/// Forwards engine events to the UI and raises attention for new held transactions.
fn bridge_events(app: AppHandle, engine: Engine) {
    let mut rx = engine.subscribe();
    tauri::async_runtime::spawn(async move {
        let mut known_pending: std::collections::HashSet<u64> = Default::default();
        loop {
            let event = match rx.recv().await {
                Ok(e) => e,
                // Events were dropped in a burst: resend the state that matters
                // (held transactions, listener) so the UI cannot go stale.
                Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                    tracing::warn!("UI bridge skipped {n} events; resyncing");
                    let _ = app.emit(EVENT_CHANNEL, &Event::Server(engine.server_status()));
                    Event::Pending(engine.pending())
                }
                Err(_) => break,
            };
            if let Event::Pending(list) = &event {
                let fresh: Vec<&RequestRecord> = list.iter().filter(|r| !known_pending.contains(&r.id)).collect();
                if let Some(first) = fresh.first() {
                    let _ = app
                        .notification()
                        .builder()
                        .title(tr("notify.held_title", &[]))
                        .body(tr("notify.held_body", &[("chain", first.chain_id.to_string()), ("summary", first.summary.clone())]))
                        .show();
                    show_main_window(&app);
                }
                known_pending = list.iter().map(|r| r.id).collect();
            }
            match &event {
                Event::SafeAlert(alert) if alert.severity >= Severity::Warning => {
                    let _ = app
                        .notification()
                        .builder()
                        .title(tr(if alert.severity == Severity::Danger { "notify.safe_alert" } else { "notify.safe_activity" }, &[]))
                        .body(format!(
                            "{} · {}",
                            txgate_core::decode::short(&alert.safe),
                            alert.text.as_ref().map(|t| t.localized(&UI_LANG.read().map(|l| l.clone()).unwrap_or_default())).unwrap_or_else(|| alert.message.clone())
                        ))
                        .show();
                }
                _ => {}
            }
            let _ = app.emit(EVENT_CHANNEL, &event);
        }
    });
}

fn setup_tray(app: &tauri::App) -> tauri::Result<()> {
    let show = MenuItem::with_id(app, "show", tr("tray.show", &[]), true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", tr("tray.quit", &[]), true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show, &quit])?;
    app.manage(TrayItems { show: show.clone(), quit: quit.clone() });
    let mut builder = TrayIconBuilder::with_id("main")
        .tooltip("TxGate")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "show" => show_main_window(app),
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = event {
                show_main_window(tray.app_handle());
            }
        });
    // macOS draws the monochrome glyph as a template so it follows the menu bar
    // color. Elsewhere the bar may be dark or light, so the full-color app icon
    // stays readable on both.
    #[cfg(target_os = "macos")]
    {
        builder = builder.icon(tauri::image::Image::from_bytes(include_bytes!("../icons/tray/trayTemplate-2x.png"))?).icon_as_template(true);
    }
    #[cfg(not(target_os = "macos"))]
    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }
    builder.build(app)?;
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();

    tauri::Builder::default()
        // A second launch would share the request log and fight over the port:
        // focus the running instance instead. Must be the first plugin.
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| show_main_window(app)))
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let config_path = app.path().app_config_dir()?.join("config.toml");
            let db_path = app.path().app_data_dir()?.join("txgate.sqlite");
            let (config, startup_warning) = load_config(&config_path);
            if !config_path.exists() {
                // Write defaults so the user can find and edit the file.
                let _ = config.save(&config_path);
            }
            // Files written by older versions may be readable by other users.
            #[cfg(unix)]
            let _ = std::fs::set_permissions(&config_path, std::os::unix::fs::PermissionsExt::from_mode(0o600));
            let (store, db_warning) = open_store(&db_path);
            let startup_warning = [startup_warning, db_warning].into_iter().flatten().reduce(|a, b| format!("{a}\n{b}"));
            let engine = Engine::new(config, Some(config_path.clone()), store, Arc::new(HttpUpstream::new()))
                .map_err(|e| Box::new(e) as Box<dyn std::error::Error>)?;

            bridge_events(app.handle().clone(), engine.clone());
            watch_safes(engine.clone());
            app.manage(AppState { engine, server: Mutex::new(None), config_path, db_path, startup_warning, exit_warned_ms: Default::default() });
            setup_tray(app)?;

            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                let state = handle.state::<AppState>();
                restart_server(&state).await;
            });
            Ok(())
        })
        .on_window_event(|window, event| {
            // Closing the window keeps the gate running in the tray.
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .invoke_handler(tauri::generate_handler![
            get_snapshot,
            decide,
            simulate,
            query_history,
            clear_history,
            set_ui_language,
            check_update,
            save_config,
            save_network,
            delete_network,
            restart_listener,
            safe_review,
            safe_queue,
            safe_poll,
            safe_alerts_read,
            set_address_note,
        ])
        .build(tauri::generate_context!())
        .expect("error while building TxGate")
        .run(|app, event| match event {
            // macOS: clicking the dock icon re-opens the hidden window.
            #[cfg(target_os = "macos")]
            RunEvent::Reopen { .. } => show_main_window(app),
            RunEvent::ExitRequested { api, .. } => {
                let state = app.state::<AppState>();
                let held = state.engine.pending().len();
                let now = txgate_core::engine::now_ms();
                let last = state.exit_warned_ms.load(std::sync::atomic::Ordering::Relaxed);
                if exit_needs_confirmation(held, last, now) {
                    api.prevent_exit();
                    state.exit_warned_ms.store(now, std::sync::atomic::Ordering::Relaxed);
                    show_main_window(app);
                    let _ = app
                        .notification()
                        .builder()
                        .title(tr("notify.quit_title", &[]))
                        .body(tr("notify.quit_body", &[("count", held.to_string())]))
                        .show();
                }
            }
            _ => {}
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quitting_with_held_transactions_needs_a_second_attempt() {
        assert!(!exit_needs_confirmation(0, 0, 50_000), "nothing held: quit at once");
        assert!(exit_needs_confirmation(2, 0, 50_000), "first attempt is refused");
        assert!(!exit_needs_confirmation(2, 45_000, 50_000), "second attempt within 10 s quits");
        assert!(exit_needs_confirmation(2, 30_000, 50_000), "a stale warning does not count");
    }

    #[test]
    fn unreadable_config_is_kept_aside_not_overwritten() {
        let dir = std::env::temp_dir().join(format!("txgate-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        std::fs::write(&path, "this is = = not toml").unwrap();

        let (config, warning) = load_config(&path);
        assert_eq!(config, Config::default());
        assert!(warning.unwrap().contains("broken-"));
        let kept: Vec<_> = std::fs::read_dir(&dir).unwrap().filter_map(|e| e.ok()).map(|e| e.file_name().into_string().unwrap()).collect();
        assert!(kept.iter().any(|n| n.starts_with("config.broken-")), "{kept:?}");
        assert!(!path.exists(), "the broken file was moved, so defaults can be written");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn unopenable_log_falls_back_to_a_fresh_one() {
        let dir = std::env::temp_dir().join(format!("txgate-db-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("txgate.sqlite");
        std::fs::write(&path, vec![0x42u8; 4096]).unwrap(); // not a SQLite file

        let (store, warning) = open_store(&path);
        assert!(warning.unwrap().contains("new log"));
        assert_eq!(store.max_id().unwrap(), 0);
        assert!(dir.join("txgate.broken.sqlite").exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
