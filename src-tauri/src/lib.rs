//! The Tauri app until the native one replaces it (stage 12): a webview over
//! `rustradar-backend`. Every command is a thin wrapper in `commands.rs`; the
//! updaters stay here, being Tauri's.

mod commands;
// the checks run in the desktop tests too; only phones use it
#[cfg(any(target_os = "android", target_os = "ios", test))]
#[cfg_attr(not(any(target_os = "android", target_os = "ios")), allow(dead_code))]
mod mobile_update;
#[cfg(not(any(target_os = "android", target_os = "ios")))]
mod updater;

use std::sync::Arc;

use rustradar_backend::{APP_ID, Backend, BackendEvent, Dirs};
use tauri::{Emitter, Manager};

/// Starts the backend in Tauri's folders (the same ones every earlier version
/// used, on phones too) and hands its events to the page as the events it
/// listens for.
fn start_backend(app: &tauri::AppHandle) -> Result<Backend, String> {
    let paths = app.path();
    // only a system without a home has none of these
    let fallback = || std::env::temp_dir().join(APP_ID);
    let dirs = Dirs {
        cache: paths.app_cache_dir().unwrap_or_else(|_| fallback()),
        config: paths.app_config_dir().unwrap_or_else(|_| fallback()),
        downloads: paths.download_dir().or_else(|_| paths.home_dir()).ok(),
    };
    let (events, heard) = std::sync::mpsc::channel();
    let handle = app.clone();
    std::thread::Builder::new()
        .name("backend-events".into())
        .spawn(move || {
            for event in heard {
                let _ = match event {
                    BackendEvent::TrainsChanged => handle.emit("trains-changed", ()),
                    BackendEvent::UpdateReady { version } => handle.emit("update-ready", version),
                };
            }
        })
        .map_err(|e| e.to_string())?;
    // the page draws itself when it hears something: nothing to wake
    Backend::start(dirs, tauri::async_runtime::handle().inner().clone(), events, Arc::new(|| {}))
}

#[cfg_attr(any(target_os = "android", target_os = "ios"), tauri::mobile_entry_point)]
pub fn run() {
    // the backend spawns its background work (tiles, platforms, signals, streams) on Tauri's runtime
    rustradar_backend::set_rt(tauri::async_runtime::handle().inner().clone());
    let builder = tauri::Builder::default().plugin(tauri_plugin_opener::init());
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    let builder = builder.plugin(tauri_plugin_updater::Builder::new().build());
    builder
        // the rail network's tiles, for the map: fetched and kept by the backend
        .register_asynchronous_uri_scheme_protocol("railtiles", |_ctx, request, responder| {
            let path = request.uri().path().to_owned();
            tauri::async_runtime::spawn(async move { responder.respond(commands::rail_tiles(&path).await) });
        })
        // buildings far out, quantized (building_lod.rs)
        .register_asynchronous_uri_scheme_protocol("buildingtiles", |_ctx, request, responder| {
            let path = request.uri().path().to_owned();
            tauri::async_runtime::spawn(async move { responder.respond(commands::building_tiles(&path).await) });
        })
        .setup(|app| {
            // the HTTP client, the folders, the saved keys, the trains' live
            // streams and the launch login; commands are only heard once this returns
            app.manage(start_backend(app.handle())?);
            // the main window starts hidden: update first, then open it
            #[cfg(not(any(target_os = "android", target_os = "ios")))]
            tauri::async_runtime::spawn(updater::run(app.handle().clone()));
            #[cfg(any(target_os = "android", target_os = "ios"))]
            tauri::async_runtime::spawn(mobile_update::run(app.handle().clone()));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::live_flights,
            commands::flight_details,
            commands::follow_flight,
            commands::unfollow_flight,
            commands::top_flights,
            commands::search,
            commands::reference_data,
            commands::airport_runways,
            commands::airport_board,
            commands::aircraft_history,
            commands::airline_colors,
            commands::radar_frame,
            commands::radar_image,
            commands::opensky_track,
            commands::adsb_hex,
            commands::adsb_callsign,
            commands::geocode,
            commands::session_info,
            commands::sign_in,
            commands::sign_out,
            commands::get_keys,
            commands::save_keys,
            commands::airport_taxiways,
            commands::save_export,
            commands::open_devtools,
            #[cfg(not(any(target_os = "android", target_os = "ios")))]
            updater::pending_update,
            #[cfg(not(any(target_os = "android", target_os = "ios")))]
            updater::restart_to_update,
            #[cfg(any(target_os = "android", target_os = "ios"))]
            mobile_update::pending_update,
            #[cfg(any(target_os = "android", target_os = "ios"))]
            mobile_update::restart_to_update,
            commands::ships_in_view,
            commands::ship_details,
            commands::trains_in_view,
            commands::train_stops,
            commands::train_route,
            commands::watch_trains,
            commands::train_blocks,
            commands::weather_at,
            commands::hide_train_operators,
            commands::dev_log,
            commands::dev_log_clear,
            commands::product_codes,
            commands::trafikverket_key_status,
            commands::road_widths,
            commands::follow_train,
            commands::unfollow_train,
            commands::train_media,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
