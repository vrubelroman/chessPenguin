//! Sign in with ChatGPT: requests paid from the user's ChatGPT plan
//! (https://developers.openai.com/siwc/token-sharing-open-source).
//!
//! OAuth with PKCE in the system browser and a loopback listener on 127.0.0.1; the
//! client is registered on the first sign-in (`dynamic_agent_client`), the issued client
//! id and this installation's host id are kept in the settings, the tokens beside them
//! in `credentials.json`. Inference: the Responses API, streamed, not stored.

use std::io::{BufRead, BufReader, Write as _};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::translate::{Failure, agent, update_credentials, with_waits};

const AUTHORIZE: &str = "https://auth.openai.com/api/accounts/authorize";
const TOKEN: &str = "https://auth.openai.com/api/accounts/oauth/token";
const DISCOVERY: &str = "https://auth.openai.com/.well-known/openid-configuration";
const API: &str = "https://api.openai.com/v1";
const RESOURCE: &str = "https://api.openai.com/v1";
const SCOPES: &str =
    "openid profile email offline_access resource.invoke chatgpt.tokens.use.direct";
/// The scope that lets requests use the plan.
const PLAN_SCOPE: &str = "chatgpt.tokens.use.direct";
const CALLBACK_PATH: &str = "/auth/callback";
/// Ports tried for the callback (any port works; the path must stay the same).
const PORTS: [u16; 4] = [1455, 1456, 1457, 0];
/// How long the browser has for the sign-in.
const SIGN_IN_TIME: Duration = Duration::from_secs(10 * 60);

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct Tokens {
    pub access: String,
    pub refresh: String,
    /// Unix seconds.
    pub expires: u64,
    pub scope: String,
    pub client_id: String,
}

impl Tokens {
    /// The plan may be used (the user granted it at consent).
    pub fn plan(&self) -> bool {
        self.scope.split_whitespace().any(|s| s == PLAN_SCOPE)
    }
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

fn random_bytes<const N: usize>() -> [u8; N] {
    let mut b = [0u8; N];
    // No randomness is a broken system; a weaker fallback still differs per attempt.
    if getrandom::fill(&mut b).is_err() {
        let t = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let h = Sha256::digest(t.to_le_bytes());
        for (i, x) in b.iter_mut().enumerate() {
            *x = h[i % h.len()];
        }
    }
    b
}

fn random_text() -> String {
    URL_SAFE_NO_PAD.encode(random_bytes::<32>())
}

/// A new installation id (`urn:uuid:` with a random UUID v4).
pub fn new_host_id() -> String {
    let mut b = random_bytes::<16>();
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let h: String = b.iter().map(|x| format!("{x:02x}")).collect();
    format!(
        "urn:uuid:{}-{}-{}-{}-{}",
        &h[0..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..32]
    )
}

fn form(pairs: &[(&str, &str)]) -> String {
    let mut s = form_urlencoded::Serializer::new(String::new());
    for (k, v) in pairs {
        s.append_pair(k, v);
    }
    s.finish()
}

/// The `email` claim of an ID token (not verified: shown only, never trusted).
fn email_of(id_token: &str) -> Option<String> {
    let payload = id_token.split('.').nth(1)?;
    let bytes = URL_SAFE_NO_PAD.decode(payload.trim_end_matches('=')).ok()?;
    let v: Value = serde_json::from_slice(&bytes).ok()?;
    v.get("email").and_then(Value::as_str).map(str::to_owned)
}

fn nonce_of(id_token: &str) -> Option<String> {
    let payload = id_token.split('.').nth(1)?;
    let bytes = URL_SAFE_NO_PAD.decode(payload.trim_end_matches('=')).ok()?;
    let v: Value = serde_json::from_slice(&bytes).ok()?;
    v.get("nonce").and_then(Value::as_str).map(str::to_owned)
}

// --- Sign-in -------------------------------------------------------------------------------

/// How a sign-in ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SignedIn {
    /// Signed in with the plan usable: the account, the issued client id.
    Ok {
        account: String,
        client_id: String,
    },
    /// Signed in, but the plan usage was not granted at consent.
    NoPlan {
        account: String,
        client_id: String,
    },
    /// The user declined in the browser.
    Declined,
    /// The browser did not come back in time.
    TimedOut,
    Failed(String),
}

pub struct SignIn {
    /// The address to open (also offered for copying when no browser opens).
    pub url: String,
    rx: Receiver<SignedIn>,
    cancel: Arc<AtomicBool>,
}

impl SignIn {
    /// Starts listening for the callback and returns the address to open in the browser.
    pub fn start(ctx: &egui::Context, host_id: &str, client_id: &str) -> Result<SignIn, String> {
        let listener = PORTS
            .iter()
            .find_map(|p| TcpListener::bind(("127.0.0.1", *p)).ok())
            .ok_or_else(|| "no free port on 127.0.0.1".to_owned())?;
        let port = listener.local_addr().map_err(|e| e.to_string())?.port();
        listener.set_nonblocking(true).map_err(|e| e.to_string())?;
        let redirect = format!("http://127.0.0.1:{port}{CALLBACK_PATH}");
        let verifier = random_text();
        let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
        let state = random_text();
        let nonce = random_text();
        let first = client_id.is_empty();
        let client = if first {
            "dynamic_agent_client"
        } else {
            client_id
        };
        let mut params = vec![
            ("client_id", client),
            ("ext_agent_host_id", host_id),
            ("response_type", "code"),
            ("redirect_uri", redirect.as_str()),
            ("scope", SCOPES),
            ("resource", RESOURCE),
            ("state", state.as_str()),
            ("nonce", nonce.as_str()),
            ("code_challenge_method", "S256"),
            ("code_challenge", challenge.as_str()),
        ];
        if first {
            params.push(("agent_name_hint", "ChessPenguin"));
        }
        let url = format!("{AUTHORIZE}?{}", form(&params));
        let (tx, rx) = channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let c = cancel.clone();
        let ctx = ctx.clone();
        let client = client.to_owned();
        std::thread::Builder::new()
            .name("chatgpt-sign-in".into())
            .spawn(move || {
                let r = wait_for_callback(&listener, &c, &state).and_then(|(code, issued)| {
                    let client = issued.unwrap_or(client);
                    exchange(&code, &verifier, &redirect, &client, &nonce)
                });
                let _ = tx.send(r.unwrap_or_else(|e| e));
                ctx.request_repaint();
            })
            .map_err(|e| e.to_string())?;
        Ok(SignIn { url, rx, cancel })
    }

    pub fn poll(&self) -> Option<SignedIn> {
        self.rx.try_recv().ok()
    }

    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

impl Drop for SignIn {
    fn drop(&mut self) {
        self.cancel();
    }
}

/// The page the browser tab shows after the sign-in.
fn page(ok: bool) -> String {
    let (title, text) = if ok {
        (
            crate::i18n::tr("✔ You are signed in to ChatGPT"),
            crate::i18n::tr(
                "Go back to ChessPenguin — the translation goes on there. You can close this tab.",
            ),
        )
    } else {
        (
            crate::i18n::tr("Sign-in could not be finished"),
            crate::i18n::tr("Go back to ChessPenguin and try again."),
        )
    };
    format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><title>ChessPenguin</title>\
         <style>body{{font-family:system-ui,sans-serif;max-width:32rem;margin:4rem auto;padding:0 1rem;color:#222}}\
         @media (prefers-color-scheme:dark){{body{{background:#1b1b1b;color:#ddd}}}}</style></head>\
         <body><h1>{title}</h1><p>{text}</p></body></html>"
    )
}

fn reply(stream: &mut TcpStream, status: &str, body: &str) {
    let _ = write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
}

/// Waits for the browser to come back to the callback: (code, issued client id).
fn wait_for_callback(
    listener: &TcpListener,
    cancel: &AtomicBool,
    state: &str,
) -> Result<(String, Option<String>), SignedIn> {
    let start = Instant::now();
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(SignedIn::Failed("cancelled".into()));
        }
        if start.elapsed() > SIGN_IN_TIME {
            return Err(SignedIn::TimedOut);
        }
        let mut stream = match listener.accept() {
            Ok((s, _)) => s,
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(100));
                continue;
            }
            Err(e) => return Err(SignedIn::Failed(e.to_string())),
        };
        let _ = stream.set_nonblocking(false);
        let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
        let mut line = String::new();
        if BufReader::new(&stream).read_line(&mut line).is_err() {
            continue;
        }
        // "GET /auth/callback?code=…&state=… HTTP/1.1"
        let target = line.split_whitespace().nth(1).unwrap_or("");
        let (path, query) = target.split_once('?').unwrap_or((target, ""));
        if path != CALLBACK_PATH {
            reply(&mut stream, "404 Not Found", "");
            continue;
        }
        let q: std::collections::HashMap<String, String> = form_urlencoded::parse(query.as_bytes())
            .into_owned()
            .collect();
        if q.get("state").map(String::as_str) != Some(state) {
            reply(&mut stream, "400 Bad Request", &page(false));
            continue;
        }
        if let Some(err) = q.get("error") {
            reply(&mut stream, "200 OK", &page(false));
            return Err(if err == "access_denied" {
                SignedIn::Declined
            } else {
                SignedIn::Failed(format!(
                    "{err}: {}",
                    q.get("error_description").map_or("", |s| s.as_str())
                ))
            });
        }
        let Some(code) = q.get("code").cloned() else {
            reply(&mut stream, "200 OK", &page(false));
            return Err(SignedIn::Failed("no code in the callback".into()));
        };
        reply(&mut stream, "200 OK", &page(true));
        return Ok((code, q.get("client_id").cloned()));
    }
}

fn token_request(body: &str) -> Result<Value, (u16, String)> {
    let mut resp = agent(Duration::from_secs(30))
        .post(TOKEN)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .send(body)
        .map_err(|e| (0, e.to_string()))?;
    let status = resp.status().as_u16();
    let text = resp.body_mut().read_to_string().unwrap_or_default();
    if status != 200 {
        return Err((status, text));
    }
    serde_json::from_str(&text).map_err(|e| (status, e.to_string()))
}

fn tokens_from(v: &Value, client_id: &str, old_refresh: &str) -> Tokens {
    let s = |k: &str| v.get(k).and_then(Value::as_str).unwrap_or("").to_owned();
    let refresh = s("refresh_token");
    Tokens {
        access: s("access_token"),
        refresh: if refresh.is_empty() {
            old_refresh.to_owned()
        } else {
            refresh
        },
        expires: now() + v.get("expires_in").and_then(Value::as_u64).unwrap_or(3600),
        scope: s("scope"),
        client_id: client_id.to_owned(),
    }
}

fn exchange(
    code: &str,
    verifier: &str,
    redirect: &str,
    client_id: &str,
    nonce: &str,
) -> Result<SignedIn, SignedIn> {
    let body = form(&[
        ("grant_type", "authorization_code"),
        ("client_id", client_id),
        ("code", code),
        ("code_verifier", verifier),
        ("redirect_uri", redirect),
        ("resource", RESOURCE),
    ]);
    let v = token_request(&body)
        .map_err(|(status, e)| SignedIn::Failed(format!("token exchange: {status} {e}")))?;
    let id_token = v.get("id_token").and_then(Value::as_str).unwrap_or("");
    if let Some(n) = nonce_of(id_token)
        && n != nonce
    {
        return Err(SignedIn::Failed(
            "the sign-in answer is not for this request".into(),
        ));
    }
    let tokens = tokens_from(&v, client_id, "");
    if tokens.access.is_empty() {
        return Err(SignedIn::Failed("no access token".into()));
    }
    let account = email_of(id_token).unwrap_or_default();
    let plan = tokens.plan();
    update_credentials(|c| c.chatgpt = Some(tokens));
    let client_id = client_id.to_owned();
    Ok(if plan {
        SignedIn::Ok { account, client_id }
    } else {
        SignedIn::NoPlan { account, client_id }
    })
}

/// Refreshes are one at a time: the refresh token rotates.
static REFRESH: Mutex<()> = Mutex::new(());

/// A valid access token, refreshed when it is about to expire.
fn access_token() -> Result<String, Failure> {
    let _one = REFRESH.lock().unwrap_or_else(|e| e.into_inner());
    let Some(t) = crate::translate::credentials().chatgpt else {
        return Err(Failure::SignedOut);
    };
    if !t.plan() {
        return Err(Failure::NotEligible("plan usage not granted".into()));
    }
    if t.expires > now() + 120 {
        return Ok(t.access);
    }
    refresh(&t)
}

fn refresh(t: &Tokens) -> Result<String, Failure> {
    let body = form(&[
        ("grant_type", "refresh_token"),
        ("client_id", &t.client_id),
        ("refresh_token", &t.refresh),
        ("resource", RESOURCE),
    ]);
    match token_request(&body) {
        Ok(v) => {
            let new = tokens_from(&v, &t.client_id, &t.refresh);
            let access = new.access.clone();
            // The scope may be omitted on refresh: the grant stays.
            let new = Tokens {
                scope: if new.scope.is_empty() {
                    t.scope.clone()
                } else {
                    new.scope
                },
                ..new
            };
            // Signed out meanwhile: the new tokens are not kept.
            update_credentials(|c| {
                if c.chatgpt.as_ref().is_some_and(|x| x.refresh == t.refresh) {
                    c.chatgpt = Some(new);
                }
            });
            Ok(access)
        }
        Err((0, e)) => Err(Failure::Offline(e)),
        Err((status, body)) => {
            let terminal = [
                "invalid_grant",
                "invalid_refresh_token",
                "token_expired",
                "refresh_token_expired",
                "refresh_token_invalidated",
                "refresh_token_reused",
            ];
            if (400..500).contains(&status) && terminal.iter().any(|c| body.contains(c)) {
                // Only this sign-in is over (not a newer one made meanwhile).
                update_credentials(|c| {
                    if c.chatgpt.as_ref().is_some_and(|x| x.refresh == t.refresh) {
                        c.chatgpt = None;
                    }
                });
                Err(Failure::SignedOut)
            } else {
                Err(Failure::Service(status, body.chars().take(300).collect()))
            }
        }
    }
}

/// Signs out: the tokens are forgotten at once, the refresh token is revoked at OpenAI
/// in the background (if that fails, the user can disconnect the app in ChatGPT's
/// settings — the link is beside the button).
pub fn sign_out() {
    let Some(t) = crate::translate::credentials().chatgpt else {
        return;
    };
    update_credentials(|c| c.chatgpt = None);
    let _ = std::thread::Builder::new()
        .name("chatgpt-sign-out".into())
        .spawn(move || revoke(&t));
}

fn revoke(t: &Tokens) -> bool {
    let body = form(&[
        ("token", &t.refresh),
        ("token_type_hint", "refresh_token"),
        ("client_id", &t.client_id),
    ]);
    // The revocation address is published in the OpenID configuration.
    let revoke = agent(Duration::from_secs(20))
        .get(DISCOVERY)
        .call()
        .ok()
        .and_then(|mut r| r.body_mut().read_to_string().ok())
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .and_then(|v| v.get("revocation_endpoint")?.as_str().map(str::to_owned));
    revoke.is_some_and(|url| {
        agent(Duration::from_secs(20))
            .post(&url)
            .header("Content-Type", "application/x-www-form-urlencoded")
            .send(&body)
            .is_ok_and(|r| r.status().as_u16() == 200)
    })
}

// --- Requests ------------------------------------------------------------------------------

/// The models this account may use (`visibility: list`).
pub fn models() -> Result<Vec<String>, Failure> {
    let token = access_token()?;
    let mut resp = agent(Duration::from_secs(30))
        .get(format!("{API}/models"))
        .header("Authorization", format!("Bearer {token}"))
        .call()
        .map_err(|e| Failure::Offline(e.to_string()))?;
    let status = resp.status().as_u16();
    let body = resp.body_mut().read_to_string().unwrap_or_default();
    if status != 200 {
        return Err(failure(status, &body));
    }
    let v: Value =
        serde_json::from_str(&body).map_err(|e| Failure::Service(status, e.to_string()))?;
    Ok(model_list(&v))
}

/// The listed models, OpenAI's preferred first. The plan's answer is
/// `{"models": [{"slug", "visibility", "priority", "supported_in_api"}]}`; the API's
/// usual `{"data": [{"id"}]}` is read too.
fn model_list(v: &Value) -> Vec<String> {
    let mut found: Vec<(i64, String)> = Vec::new();
    for (key, name) in [("models", "slug"), ("data", "id")] {
        for m in v.get(key).and_then(Value::as_array).into_iter().flatten() {
            let listed = m
                .get("visibility")
                .and_then(Value::as_str)
                .is_none_or(|s| s == "list");
            let api = m
                .get("supported_in_api")
                .and_then(Value::as_bool)
                .unwrap_or(true);
            if let (true, true, Some(id)) = (listed, api, m.get(name).and_then(Value::as_str)) {
                let priority = m
                    .get("priority")
                    .and_then(Value::as_i64)
                    .unwrap_or(i64::MAX);
                found.push((priority, id.to_owned()));
            }
        }
    }
    found.sort_by_key(|(p, _)| *p);
    found.into_iter().map(|(_, id)| id).collect()
}

/// The model to start with: a small fast one if listed (translation needs no more).
pub fn default_model(models: &[String]) -> Option<String> {
    // OpenAI's preferred chat model (the list comes in its order); models for other
    // work (code review, search, sound, pictures) do not take a plain text request.
    const OTHER: [&str; 12] = [
        "codex",
        "search",
        "research",
        "audio",
        "realtime",
        "image",
        "tts",
        "transcribe",
        "embedding",
        "moderation",
        "computer",
        "review",
    ];
    models
        .iter()
        .find(|m| {
            let m = m.to_lowercase();
            !OTHER.iter().any(|w| m.contains(w))
        })
        .or_else(|| models.first())
        .cloned()
}

fn failure(status: u16, body: &str) -> Failure {
    let v: Option<Value> = serde_json::from_str(body).ok();
    let code = v
        .as_ref()
        .and_then(|v| v.pointer("/error/code"))
        .and_then(Value::as_str)
        .unwrap_or("");
    let message = v
        .as_ref()
        .and_then(|v| v.pointer("/error/message").or_else(|| v.get("detail")))
        .and_then(Value::as_str)
        .unwrap_or(body)
        .chars()
        .take(300)
        .collect::<String>();
    match code {
        "subscription_sharing_usage_limit_exceeded" => Failure::PlanLimit,
        "subscription_sharing_user_not_eligible" => Failure::NotEligible(message),
        "subscription_sharing_invalid_user" => Failure::SignedOut,
        "subscription_sharing_usage_unavailable" | "subscription_sharing_user_unavailable" => {
            Failure::Service(503, message)
        }
        _ => match status {
            401 => Failure::SignedOut,
            403 => Failure::NotEligible(message),
            429 => Failure::RateLimited,
            _ => Failure::Service(status, message),
        },
    }
}

/// One answer of the model, read from the event stream.
pub fn respond(
    model: &str,
    system: &str,
    user: &str,
    cancel: &AtomicBool,
) -> Result<String, Failure> {
    let mut body = json!({
        "model": model,
        "instructions": system,
        "input": [{"role": "user", "content": [{"type": "input_text", "text": user}]}],
        "store": false,
        "stream": true,
        // Translating needs no long thought: at the default effort the model thinks for
        // most of a minute over each request before it writes a word.
        "reasoning": {"effort": "low"},
    });
    let mut refreshed = false;
    with_waits(cancel, || {
        loop {
            let token = access_token()?;
            let resp = agent(Duration::from_secs(300))
                .post(format!("{API}/responses"))
                .header("Authorization", format!("Bearer {token}"))
                .header("Content-Type", "application/json")
                .header("Accept", "text/event-stream")
                .send(&body.to_string())
                .map_err(|e| Failure::Offline(e.to_string()))?;
            let status = resp.status().as_u16();
            if status == 401 && !refreshed {
                // The token may have been revoked or expired early: refresh once.
                refreshed = true;
                let _one = REFRESH.lock().unwrap_or_else(|e| e.into_inner());
                // Read under the lock: another request may have refreshed already.
                if let Some(t) = crate::translate::credentials().chatgpt
                    && t.access == token
                {
                    refresh(&t)?;
                }
                continue;
            }
            if status != 200 {
                let mut resp = resp;
                let text = resp.body_mut().read_to_string().unwrap_or_default();
                // A model that does not think takes no effort: asked again without it.
                if status == 400
                    && text.contains("reasoning")
                    && body
                        .as_object_mut()
                        .and_then(|b| b.remove("reasoning"))
                        .is_some()
                {
                    continue;
                }
                return Err(failure(status, &text));
            }
            return read_stream(BufReader::new(resp.into_body().into_reader()), cancel);
        }
    })
}

/// The text of a Responses event stream: the deltas until `response.completed`.
fn read_stream(reader: impl BufRead, cancel: &AtomicBool) -> Result<String, Failure> {
    let mut text = String::new();
    for line in reader.lines() {
        if cancel.load(Ordering::Relaxed) {
            return Err(Failure::Offline("cancelled".into()));
        }
        let line = line.map_err(|e| Failure::Offline(e.to_string()))?;
        let Some(data) = line.strip_prefix("data:") else {
            continue;
        };
        let data = data.trim();
        if data == "[DONE]" {
            break;
        }
        let Ok(v) = serde_json::from_str::<Value>(data) else {
            continue;
        };
        match v.get("type").and_then(Value::as_str).unwrap_or("") {
            "response.output_text.delta" => {
                if let Some(d) = v.get("delta").and_then(Value::as_str) {
                    text.push_str(d);
                }
            }
            "response.completed" => return Ok(text),
            "response.failed" | "error" => {
                let err = v
                    .pointer("/response/error")
                    .or_else(|| v.get("error"))
                    .cloned()
                    .unwrap_or(Value::Null);
                return Err(failure(0, &json!({ "error": err }).to_string()));
            }
            "response.incomplete" => {
                return Err(Failure::Service(0, "the answer was cut short".into()));
            }
            _ => {}
        }
    }
    Err(Failure::Offline("the answer stream ended early".into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read as _;

    #[test]
    fn host_ids_are_uuid_urns() {
        let id = new_host_id();
        assert!(id.starts_with("urn:uuid:"), "{id}");
        let u = &id[9..];
        assert_eq!(u.len(), 36);
        assert_eq!(&u[14..15], "4");
        assert_ne!(new_host_id(), id);
    }

    #[test]
    fn a_stream_gives_its_text() {
        let s = "event: response.created\ndata: {\"type\":\"response.created\"}\n\n\
                 data: {\"type\":\"response.output_text.delta\",\"delta\":\"{\\\"translations\\\"\"}\n\n\
                 data: {\"type\":\"response.output_text.delta\",\"delta\":\": []}\"}\n\n\
                 data: {\"type\":\"response.completed\",\"response\":{}}\n\n";
        let never = AtomicBool::new(false);
        assert_eq!(
            read_stream(s.as_bytes(), &never).unwrap(),
            "{\"translations\": []}"
        );
    }

    #[test]
    fn a_used_up_limit_is_told_apart() {
        let s = "data: {\"type\":\"response.failed\",\"response\":{\"error\":{\"code\":\"subscription_sharing_usage_limit_exceeded\",\"message\":\"limit\"}}}\n\n";
        let never = AtomicBool::new(false);
        assert_eq!(read_stream(s.as_bytes(), &never), Err(Failure::PlanLimit));
        assert_eq!(
            failure(
                403,
                r#"{"error":{"code":"subscription_sharing_user_not_eligible","message":"free plan"}}"#
            ),
            Failure::NotEligible("free plan".into())
        );
        assert_eq!(failure(401, r#"{"detail":"bad"}"#), Failure::SignedOut);
    }

    #[test]
    fn a_cut_stream_is_not_an_answer() {
        let s = "data: {\"type\":\"response.output_text.delta\",\"delta\":\"half\"}\n\n";
        let never = AtomicBool::new(false);
        assert!(read_stream(s.as_bytes(), &never).is_err());
    }

    #[test]
    fn the_email_is_read_from_the_id_token() {
        let payload = URL_SAFE_NO_PAD.encode(br#"{"email":"a@b.c","nonce":"n1"}"#);
        let t = format!("x.{payload}.y");
        assert_eq!(email_of(&t).as_deref(), Some("a@b.c"));
        assert_eq!(nonce_of(&t).as_deref(), Some("n1"));
    }

    #[test]
    fn the_plan_model_list_is_read() {
        // The shape the plan's /v1/models answers with (seen 2026-10-21).
        let v: Value = serde_json::from_str(
            r#"{"models": [
                {"slug": "gpt-6-luna", "visibility": "list", "priority": 5, "supported_in_api": true},
                {"slug": "codex-auto-review", "visibility": "hide", "priority": 0, "supported_in_api": true},
                {"slug": "gpt-6.1-sol", "visibility": "list", "priority": 1, "supported_in_api": true},
                {"slug": "gpt-reserve", "visibility": "hide", "priority": 3}
            ]}"#,
        )
        .unwrap();
        let list = model_list(&v);
        assert_eq!(list, vec!["gpt-6.1-sol", "gpt-6-luna"]);
        assert_eq!(default_model(&list).as_deref(), Some("gpt-6.1-sol"));
        let api: Value = serde_json::from_str(r#"{"data": [{"id": "gpt-x"}]}"#).unwrap();
        assert_eq!(model_list(&api), vec!["gpt-x"]);
        let m = |v: &[&str]| default_model(&v.iter().map(|s| s.to_string()).collect::<Vec<_>>());
        assert_eq!(
            m(&["gpt-6.1-codex", "gpt-6.1-sol"]),
            Some("gpt-6.1-sol".into())
        );
        assert_eq!(m(&[]), None);
    }

    #[test]
    fn the_plan_scope_is_checked() {
        let t = Tokens {
            scope: "openid email chatgpt.tokens.use.direct".into(),
            ..Default::default()
        };
        assert!(t.plan());
        assert!(!Tokens::default().plan());
    }

    #[test]
    fn the_callback_brings_the_code() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        listener.set_nonblocking(true).unwrap();
        let port = listener.local_addr().unwrap().port();
        let browser = std::thread::spawn(move || {
            let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
            write!(
                s,
                "GET /auth/callback?code=abc&state=st&client_id=oaiapp_1 HTTP/1.1\r\nHost: x\r\n\r\n"
            )
            .unwrap();
            let mut answer = String::new();
            let _ = s.read_to_string(&mut answer);
            answer
        });
        let never = AtomicBool::new(false);
        let got = wait_for_callback(&listener, &never, "st").unwrap();
        assert_eq!(got, ("abc".to_string(), Some("oaiapp_1".to_string())));
        assert!(browser.join().unwrap().starts_with("HTTP/1.1 200"));
    }

    #[test]
    fn a_declined_consent_is_told_apart() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        listener.set_nonblocking(true).unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
            write!(
                s,
                "GET /auth/callback?error=access_denied&state=st HTTP/1.1\r\n\r\n"
            )
            .unwrap();
            let mut a = String::new();
            let _ = s.read_to_string(&mut a);
        });
        let never = AtomicBool::new(false);
        assert_eq!(
            wait_for_callback(&listener, &never, "st"),
            Err(SignedIn::Declined)
        );
    }
}
