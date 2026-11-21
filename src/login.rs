//! Background device-code login.
//!
//! The UI thread must keep drawing while Microsoft waits for the user, so the
//! whole flow runs on its own thread with its own current-thread runtime and
//! reports progress over a channel. Nothing here touches the UI, and the tokens
//! only ever exist in this thread and in the account file it writes — they are
//! never sent to the UI.
//!
//! Ported from `tessera-core::login`. The flow is device-code only: it needs no
//! redirect URI, which is exactly why it fits a phone — the browser is the
//! system's, not an embedded one.

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender};

use minecraft_auth::java::JavaAuthManager;
use minecraft_auth::AuthApplication;

use crate::accounts;

/// Which application identity the sign-in runs as.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Identity {
    /// The official launcher's title client id (device token + SISU).
    Official,
    /// One of the ids in `azure_app_ids.toml`, signed in under the name that
    /// file gives it.
    Saved { name: String, app_id: String },
}

impl Identity {
    /// The label written into the account file, i.e. what the list shows.
    pub fn label(&self) -> String {
        match self {
            Identity::Official => "官方启动器".to_string(),
            Identity::Saved { name, app_id } => {
                let name = name.trim();
                if name.is_empty() {
                    app_id.trim().to_string()
                } else {
                    name.to_string()
                }
            }
        }
    }
}

/// What the login thread tells the UI.
#[derive(Debug)]
pub enum Event {
    /// The user must open `url` and enter `user_code`.
    DeviceCode { user_code: String, url: String },
    Succeeded {
        player_name: String,
        application: String,
        file: PathBuf,
    },
    Failed(String),
}

/// Start the flow. The returned receiver ends when the thread finishes.
///
/// A sender that closes without a terminal event means the thread died; the
/// caller is expected to say so rather than wait forever.
pub fn spawn(identity: Identity, accounts_dir: PathBuf) -> Receiver<Event> {
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let outcome = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|error| format!("无法创建异步运行时: {error}"))
            .and_then(|runtime| runtime.block_on(run(identity, accounts_dir, sender.clone())));
        if let Err(problem) = outcome {
            let _ = sender.send(Event::Failed(problem));
        }
    });
    receiver
}

/// A client that trusts a bundled root store.
///
/// reqwest's `rustls` feature verifies through `rustls-platform-verifier`, and
/// that crate panics on Android until it has been initialised with a JNI
/// context ("Expect rustls-platform-verifier to be initialized"). Supplying the
/// config ourselves keeps the sign-in free of JNI and gives the same trust
/// decisions on a phone and on a desktop, at the cost of ignoring CAs the user
/// installed.
fn http_client() -> Result<reqwest::Client, String> {
    let mut roots = rustls::RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());

    let provider = std::sync::Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let config = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|error| format!("TLS 配置失败: {error}"))?
        .with_root_certificates(roots)
        .with_no_client_auth();

    reqwest::Client::builder()
        .user_agent(concat!(
            "tessera-launcher/",
            env!("CARGO_PKG_VERSION"),
            " (+https://github.com/Tessera-Launcher/Tessera)"
        ))
        .use_preconfigured_tls(config)
        .build()
        .map_err(|error| format!("无法创建 HTTP 客户端: {error}"))
}

async fn run(
    identity: Identity,
    accounts_dir: PathBuf,
    sender: Sender<Event>,
) -> Result<(), String> {
    let application = match &identity {
        Identity::Official => AuthApplication::official_launcher(),
        Identity::Saved { name, app_id } => {
            // `azure_application` rejects a blank or malformed id before any
            // request is made, so a bad file entry fails without a device code.
            AuthApplication::azure_application(name.trim(), app_id.trim())
        }
    };
    let application_label = application.label.clone();
    let app_id = application.config.client_id.clone();

    let client = http_client()?;

    let code_sender = sender.clone();
    let manager = JavaAuthManager::builder(client)
        .application(&application)
        .login_device_code(move |code| {
            let _ = code_sender.send(Event::DeviceCode {
                user_code: code.user_code.clone(),
                url: code.direct_verification_uri(),
            });
        })
        .await
        .map_err(|error| format!("登录失败: {error}"))?;

    // `launch_session` resolves the profile (name + uuid) and the access token.
    let session = manager
        .launch_session()
        .await
        .map_err(|error| format!("获取游戏档案失败: {error}"))?;
    let document = manager
        .to_json()
        .await
        .map_err(|error| format!("序列化会话失败: {error}"))?;

    let file = accounts::save(
        &accounts_dir,
        &application_label,
        &app_id,
        &session.player_name,
        &session.player_uuid.to_string(),
        &document,
    )
    .map_err(|error| format!("写入凭据失败: {error}"))?;

    let _ = sender.send(Event::Succeeded {
        player_name: session.player_name,
        application: application_label,
        file,
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identities_have_distinct_labels() {
        assert_eq!(Identity::Official.label(), "官方启动器");
        // A saved entry is labelled by the name in the file, so the account list
        // says `HMCL` rather than a generic string.
        assert_eq!(
            Identity::Saved {
                name: " HMCL ".into(),
                app_id: "6a3728d6-27a3-4180-99bb-479895b8f88e".into()
            }
            .label(),
            "HMCL"
        );
    }

    #[test]
    fn a_nameless_saved_entry_falls_back_to_its_id() {
        assert_eq!(
            Identity::Saved {
                name: "   ".into(),
                app_id: " bb ".into()
            }
            .label(),
            "bb"
        );
    }

    #[test]
    fn the_event_shape_the_ui_reads_is_stable() {
        // Only the channel plumbing is exercised; `spawn` would hit Microsoft.
        let (sender, receiver) = std::sync::mpsc::channel();
        sender
            .send(Event::DeviceCode {
                user_code: "ABCD-EFGH".into(),
                url: "https://microsoft.com/link".into(),
            })
            .unwrap();
        drop(sender);

        match receiver.recv().unwrap() {
            Event::DeviceCode { user_code, url } => {
                assert_eq!(user_code, "ABCD-EFGH");
                assert!(url.starts_with("https://"));
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }
}
