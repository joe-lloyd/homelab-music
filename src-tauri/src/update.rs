// Self-updating, on the app's terms.
//
// Tauri's updater downloads the whole bundle rather than a delta. That sounds
// wasteful and is not: these builds are 3-8 MB, so a full replacement costs
// less than the machinery to avoid one would.
//
// When it installs is the whole design. Installing means relaunching, and
// relaunching mid-album to save someone thirty seconds is a bad trade. So the
// startup check installs -- nothing is playing yet, the window has just
// opened, and a relaunch there costs nothing. A check that finds an update
// later notifies instead and waits to be told, from the tray or from settings.
//
// The effect is that the app updates itself without being asked, on the next
// launch after a release, which is what "auto-update" has to mean for
// something that lives in the tray for weeks at a time.

use tauri::{AppHandle, Manager};
use tauri_plugin_notification::NotificationExt;
use tauri_plugin_updater::UpdaterExt;

/// Told to the user, so it says what happened rather than what failed.
fn notify(app: &AppHandle, title: &str, body: &str) {
    let _ = app.notification().builder().title(title).body(body).show();
}

/// What a check found, for a caller that shows it rather than notifies it.
#[derive(Clone, serde::Serialize)]
#[serde(tag = "state", rename_all = "kebab-case")]
pub enum Found {
    Current { version: String },
    Available { version: String },
    Failed { error: String },
}

/// Look for a newer release.
///
/// `announce_when_current` separates the callers: one that asked a question
/// deserves an answer either way, while a background check should stay quiet
/// unless it has news. `auto_install` is the startup path, where relaunching
/// is free because nothing is playing yet.
pub async fn check(app: AppHandle, announce_when_current: bool, auto_install: bool) -> Found {
    let updater = match app.updater() {
        Ok(u) => u,
        Err(e) => {
            log::warn!("updater unavailable: {e}");
            if announce_when_current {
                notify(&app, "Could not check for updates", &e.to_string());
            }
            return Found::Failed {
                error: e.to_string(),
            };
        }
    };

    match updater.check().await {
        Ok(Some(update)) => {
            let version = update.version.clone();
            log::info!("update available: {version}");
            app.state::<PendingUpdate>().set(Some(update));

            if auto_install {
                log::info!("installing {version} at startup");
                install(app).await;
                return Found::Available { version };
            }

            notify(
                &app,
                &format!("Homelab Music {version} is available"),
                "Install it from settings or the tray when you are not listening to something.",
            );
            Found::Available { version }
        }
        Ok(None) => {
            log::info!("already on the latest version");
            if announce_when_current {
                notify(
                    &app,
                    "Homelab Music is up to date",
                    env!("CARGO_PKG_VERSION"),
                );
            }
            Found::Current {
                version: env!("CARGO_PKG_VERSION").to_string(),
            }
        }
        Err(e) => {
            // A failed check is not worth interrupting anyone over unless they
            // asked. Off the LAN and outside the tunnel this simply cannot
            // reach GitHub, which is an ordinary state, not a fault.
            log::warn!("update check failed: {e}");
            if announce_when_current {
                notify(&app, "Could not check for updates", &e.to_string());
            }
            Found::Failed {
                error: e.to_string(),
            }
        }
    }
}

/// Install whatever the last check found, then relaunch.
pub async fn install(app: AppHandle) {
    let Some(update) = app.state::<PendingUpdate>().take() else {
        notify(&app, "Nothing to install", "Check for updates first.");
        return;
    };

    let version = update.version.clone();
    notify(
        &app,
        &format!("Installing {version}"),
        "The app will restart when it is done.",
    );

    match update
        .download_and_install(|_chunk, _total| {}, || {})
        .await
    {
        Ok(()) => {
            log::info!("installed {version}, restarting");
            app.restart();
        }
        Err(e) => {
            log::error!("update install failed: {e}");
            notify(&app, "Update failed", &e.to_string());
        }
    }
}

/// Holds the update between finding it and being told to install it.
#[derive(Default)]
pub struct PendingUpdate(std::sync::Mutex<Option<tauri_plugin_updater::Update>>);

impl PendingUpdate {
    fn set(&self, update: Option<tauri_plugin_updater::Update>) {
        if let Ok(mut slot) = self.0.lock() {
            *slot = update;
        }
    }

    fn take(&self) -> Option<tauri_plugin_updater::Update> {
        self.0.lock().ok().and_then(|mut slot| slot.take())
    }

    /// Whether a check has found something waiting, for the tray label.
    pub fn is_pending(&self) -> bool {
        self.0.lock().map(|slot| slot.is_some()).unwrap_or(false)
    }
}
