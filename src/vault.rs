//! Зашифрованное хранилище: api_id/api_hash и сессия Telegram лежат в одном файле,
//! зашифрованном паролем (Argon2id -> ChaCha20-Poly1305). Без пароля прочитать нельзя.

use std::convert::Infallible;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use anyhow::{Context, Result, anyhow};
use argon2::{Algorithm, Argon2, Params, Version};
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{ChaCha20Poly1305, Nonce};
use grammers_session::types::{ChannelState, DcOption, PeerId, PeerInfo, UpdateState, UpdatesState};
use grammers_session::{BoxFuture, Session, SessionData};
use serde::{Deserialize, Serialize};
use zeroize::Zeroize;

use crate::settings::Settings;

const MAGIC: &[u8; 6] = b"TGLT01";
const SALT_LEN: usize = 16;
const NONCE_LEN: usize = 12;

/// Всё, что хранится в файле (в расшифрованном виде).
#[derive(Serialize, Deserialize, Clone)]
pub struct Secrets {
    pub api_id: i32,
    pub api_hash: String,
    pub session: SessionBlob,
    /// Нет в хранилищах версий 0.0.1/0.0.2: тогда берутся значения по умолчанию.
    #[serde(default)]
    pub settings: Settings,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct SessionBlob {
    pub home_dc: i32,
    pub dc_options: Vec<DcOption>,
    pub peers: Vec<PeerInfo>,
    pub updates_state: UpdatesState,
}

impl Default for SessionBlob {
    fn default() -> Self {
        SessionBlob::from(&SessionData::default())
    }
}

impl From<&SessionData> for SessionBlob {
    fn from(d: &SessionData) -> Self {
        SessionBlob {
            home_dc: d.home_dc,
            dc_options: d.dc_options.values().cloned().collect(),
            peers: d.peer_infos.values().cloned().collect(),
            updates_state: d.updates_state.clone(),
        }
    }
}

impl From<SessionBlob> for SessionData {
    fn from(b: SessionBlob) -> Self {
        let mut d = SessionData::default();
        d.home_dc = b.home_dc;
        for o in b.dc_options {
            d.dc_options.insert(o.id, o);
        }
        for p in b.peers {
            d.peer_infos.insert(p.id(), p);
        }
        d.updates_state = b.updates_state;
        d
    }
}

// ---------------------------------------------------------------- шифрование

fn derive_key(password: &str, salt: &[u8]) -> Result<[u8; 32]> {
    // ~19 МБ памяти и 2 прохода: надёжно, но не тяжело для слабого ПК.
    let params = Params::new(19 * 1024, 2, 1, Some(32)).map_err(|e| anyhow!("argon2: {e}"))?;
    let argon = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
    let mut key = [0u8; 32];
    argon
        .hash_password_into(password.as_bytes(), salt, &mut key)
        .map_err(|e| anyhow!("argon2: {e}"))?;
    Ok(key)
}

fn random_bytes<const N: usize>() -> Result<[u8; N]> {
    let mut buf = [0u8; N];
    getrandom::fill(&mut buf).map_err(|e| anyhow!("не удалось получить случайные байты: {e}"))?;
    Ok(buf)
}

pub fn encrypt(secrets: &Secrets, password: &str) -> Result<Vec<u8>> {
    let mut plain = serde_json::to_vec(secrets)?;
    let salt = random_bytes::<SALT_LEN>()?;
    let nonce = random_bytes::<NONCE_LEN>()?;
    let mut key = derive_key(password, &salt)?;
    let cipher = ChaCha20Poly1305::new_from_slice(&key).map_err(|_| anyhow!("неверная длина ключа"))?;
    // Заголовок (магия + соль + nonce) защищён как associated data: подмена заметна.
    let mut header = Vec::with_capacity(MAGIC.len() + SALT_LEN + NONCE_LEN);
    header.extend_from_slice(MAGIC);
    header.extend_from_slice(&salt);
    header.extend_from_slice(&nonce);
    let ct = cipher
        .encrypt(
            &Nonce::try_from(&nonce[..]).map_err(|_| anyhow!("nonce"))?,
            Payload { msg: plain.as_slice(), aad: &header },
        )
        .map_err(|_| anyhow!("ошибка шифрования"))?;
    key.zeroize();
    plain.zeroize();

    let mut out = header;
    out.extend_from_slice(&ct);
    Ok(out)
}

pub fn decrypt(data: &[u8], password: &str) -> Result<Secrets> {
    let head = MAGIC.len() + SALT_LEN + NONCE_LEN;
    if data.len() < head + 16 || &data[..MAGIC.len()] != MAGIC {
        return Err(anyhow!("файл хранилища повреждён или имеет неизвестный формат"));
    }
    let salt = &data[MAGIC.len()..MAGIC.len() + SALT_LEN];
    let nonce = &data[MAGIC.len() + SALT_LEN..head];
    let mut key = derive_key(password, salt)?;
    let cipher = ChaCha20Poly1305::new_from_slice(&key).map_err(|_| anyhow!("неверная длина ключа"))?;
    let plain = cipher.decrypt(
        &Nonce::try_from(nonce).map_err(|_| anyhow!("nonce"))?,
        Payload { msg: &data[head..], aad: &data[..head] },
    );
    key.zeroize();
    let mut plain = plain.map_err(|_| anyhow!("неверный пароль"))?;
    let secrets = serde_json::from_slice(&plain).context("содержимое хранилища не читается");
    plain.zeroize();
    secrets
}

// ---------------------------------------------------------------- файл на диске

pub struct VaultFile {
    path: PathBuf,
}

impl VaultFile {
    pub fn new(path: PathBuf) -> Self {
        VaultFile { path }
    }

    pub fn exists(&self) -> bool {
        self.path.exists()
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn load(&self, password: &str) -> Result<Secrets> {
        let data = std::fs::read(&self.path)
            .with_context(|| format!("не удалось прочитать {}", self.path.display()))?;
        decrypt(&data, password)
    }

    /// Атомарная запись: сначала во временный файл, потом переименование,
    /// чтобы обрыв питания не оставил битое хранилище.
    pub fn save(&self, secrets: &Secrets, password: &str) -> Result<()> {
        let bytes = encrypt(secrets, password)?;
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = self.path.with_extension("tmp");
        std::fs::write(&tmp, &bytes)?;
        std::fs::rename(&tmp, &self.path)?;
        Ok(())
    }
}

// ---------------------------------------------------------------- сессия для grammers

/// Сессия в памяти, из которой можно в любой момент получить снимок для сохранения.
pub struct VaultSession(Mutex<SessionData>);

impl VaultSession {
    pub fn from_blob(blob: SessionBlob) -> Self {
        VaultSession(Mutex::new(blob.into()))
    }

    pub fn snapshot(&self) -> SessionBlob {
        SessionBlob::from(&*self.data())
    }

    fn data(&self) -> MutexGuard<'_, SessionData> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl Session for VaultSession {
    type Error = Infallible;

    fn home_dc_id(&self) -> Result<i32, Infallible> {
        Ok(self.data().home_dc)
    }

    fn set_home_dc_id(&self, dc_id: i32) -> BoxFuture<'_, Result<(), Infallible>> {
        Box::pin(async move {
            self.data().home_dc = dc_id;
            Ok(())
        })
    }

    fn dc_option(&self, dc_id: i32) -> Result<Option<DcOption>, Infallible> {
        Ok(self.data().dc_options.get(&dc_id).cloned())
    }

    fn set_dc_option(&self, dc_option: &DcOption) -> BoxFuture<'_, Result<(), Infallible>> {
        let dc_option = dc_option.clone();
        Box::pin(async move {
            self.data().dc_options.insert(dc_option.id, dc_option);
            Ok(())
        })
    }

    fn peer(&self, peer: PeerId) -> BoxFuture<'_, Result<Option<PeerInfo>, Infallible>> {
        Box::pin(async move { Ok(self.data().peer_infos.get(&peer).cloned()) })
    }

    fn cache_peer(&self, peer: PeerInfo) -> BoxFuture<'_, Result<(), Infallible>> {
        Box::pin(async move {
            let mut data = self.data();
            data.peer_infos
                .entry(peer.id())
                .or_insert_with(|| peer.clone())
                .extend_info(&peer);
            Ok(())
        })
    }

    fn updates_state(&self) -> BoxFuture<'_, Result<UpdatesState, Infallible>> {
        Box::pin(async move { Ok(self.data().updates_state.clone()) })
    }

    fn set_update_state(&self, update: UpdateState) -> BoxFuture<'_, Result<(), Infallible>> {
        Box::pin(async move {
            let mut data = self.data();
            match update {
                UpdateState::All(s) => data.updates_state = s,
                UpdateState::Primary { pts, date, seq } => {
                    data.updates_state.pts = pts;
                    data.updates_state.date = date;
                    data.updates_state.seq = seq;
                }
                UpdateState::Secondary { qts } => data.updates_state.qts = qts,
                UpdateState::Channel { id, pts } => {
                    data.updates_state.channels.retain(|c| c.id != id);
                    data.updates_state.channels.push(ChannelState { id, pts });
                }
            }
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Secrets {
        Secrets {
            api_id: 12345,
            api_hash: "abcdef0123456789".into(),
            session: SessionBlob::default(),
            settings: Settings::default(),
        }
    }

    #[test]
    fn roundtrip_with_right_password() {
        let enc = encrypt(&sample(), "correct horse").unwrap();
        let dec = decrypt(&enc, "correct horse").unwrap();
        assert_eq!(dec.api_id, 12345);
        assert_eq!(dec.api_hash, "abcdef0123456789");
        assert_eq!(dec.session.dc_options.len(), 5);
    }

    #[test]
    fn old_format_without_settings_loads() {
        // Секреты, записанные версиями 0.0.1/0.0.2, не содержат поля settings.
        let mut v = serde_json::to_value(sample()).unwrap();
        v.as_object_mut().unwrap().remove("settings");
        let s: Secrets = serde_json::from_value(v).unwrap();
        assert_eq!(s.settings, Settings::default());
        assert_eq!(s.api_id, 12345);
    }

    #[test]
    fn wrong_password_is_rejected() {
        let enc = encrypt(&sample(), "correct horse").unwrap();
        let err = decrypt(&enc, "wrong").err().unwrap();
        assert!(err.to_string().contains("неверный пароль"));
    }

    #[test]
    fn tampering_is_detected() {
        let mut enc = encrypt(&sample(), "pw").unwrap();
        let last = enc.len() - 1;
        enc[last] ^= 1;
        assert!(decrypt(&enc, "pw").is_err());
    }

    #[test]
    fn api_hash_not_visible_in_file() {
        let enc = encrypt(&sample(), "pw").unwrap();
        let hay = String::from_utf8_lossy(&enc);
        assert!(!hay.contains("abcdef0123456789"));
    }

    #[test]
    fn file_save_and_load() {
        let dir = std::env::temp_dir().join(format!("tglt-test-{}", std::process::id()));
        let f = VaultFile::new(dir.join("vault.bin"));
        f.save(&sample(), "pw").unwrap();
        assert!(f.exists());
        assert_eq!(f.load("pw").unwrap().api_id, 12345);
        let _ = std::fs::remove_dir_all(dir);
    }
}

/// Всё необходимое, чтобы в любой момент сохранить хранилище заново
/// (после входа, при смене настроек или пароля, периодически и при выходе).
pub struct Saver {
    vault: VaultFile,
    api_id: i32,
    api_hash: String,
    session: std::sync::Arc<VaultSession>,
    state: Mutex<SaverState>,
    /// Не даёт двум сохранениям одновременно писать один и тот же временный файл.
    write_lock: Mutex<()>,
}

struct SaverState {
    password: String,
    settings: Settings,
}

impl Saver {
    pub fn new(
        vault: VaultFile,
        api_id: i32,
        api_hash: String,
        session: std::sync::Arc<VaultSession>,
        password: String,
        settings: Settings,
    ) -> Self {
        Saver {
            vault,
            api_id,
            api_hash,
            session,
            state: Mutex::new(SaverState { password, settings }),
            write_lock: Mutex::new(()),
        }
    }

    fn state(&self) -> MutexGuard<'_, SaverState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn password(&self) -> String {
        self.state().password.clone()
    }

    pub fn set_password(&self, password: String) {
        self.state().password = password;
    }

    pub fn settings(&self) -> Settings {
        self.state().settings.clone()
    }

    pub fn set_settings(&self, settings: Settings) {
        self.state().settings = settings;
    }

    pub fn save(&self) -> Result<()> {
        let (password, settings) = {
            let st = self.state();
            (st.password.clone(), st.settings.clone())
        };
        let secrets = Secrets {
            api_id: self.api_id,
            api_hash: self.api_hash.clone(),
            session: self.session.snapshot(),
            settings,
        };
        let _guard = self.write_lock.lock().unwrap_or_else(|e| e.into_inner());
        self.vault.save(&secrets, &password)
    }

    /// Сохраняет в фоне, не задерживая интерфейс (Argon2 на слабом ПК занимает заметное время).
    pub fn save_in_background(self: &std::sync::Arc<Self>) {
        let s = std::sync::Arc::clone(self);
        std::thread::spawn(move || {
            let _ = s.save();
        });
    }
}
