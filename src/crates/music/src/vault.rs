//! Tracks kept on disk, so the next play needs no download: the ones that played, in a cache
//! with a size limit.
//!
//! A kept track is never a file another program can play. Each one is sealed with
//! ChaCha20-Poly1305 under a key derived from a master key, so the folder read without that key
//! is noise. File names are keyed hashes, so the folder does not say which tracks are in it
//! either. The master key lives in the OS keychain, or on a desktop with no keychain in a key
//! file readable by the owner alone.
//!
//! What goes in is what the provider sent, before its [`Body`](crate::stream::Body) touched it.
//! Peeling off the vault's layer therefore leaves only the stream as it arrived, nothing easier
//! to play and nothing that outlives the account.
//!
//! The owner of the machine can always read the master key out of their keychain or key file,
//! or the audio out of the process. The vault keeps kept tracks from playing outside Wisp for
//! anyone without the master key, and nothing more.
//!
//! An entry is the file's random salt, then the track in sealed chunks of [`CHUNK`] bytes, then
//! the provider's facts about the track sealed on their own, then the length of that last block.
//! Every chunk's nonce carries its index and whether it is the last, so a truncated, reordered or
//! swapped entry fails to open rather than playing wrong.

use std::fmt;
use std::fs::{self, File, FileTimes, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, SystemTime};

use anyhow::{Context as _, Result, anyhow, bail};
use bytes::Bytes;
use ring::aead::{Aad, CHACHA20_POLY1305, LessSafeKey, NONCE_LEN, Nonce, UnboundKey};
use ring::rand::{SecureRandom as _, SystemRandom};
use ring::{hkdf, hmac};
use tokio::sync::watch;

use sound::stream::Source;

/// How much of a track one sealed chunk holds. Big enough that the tags cost nothing, small
/// enough that a damaged chunk is found long before the decoder needs it.
const CHUNK: usize = 64 * 1024;
/// The bytes of a ChaCha20-Poly1305 tag.
const TAG: usize = 16;
/// The bytes of the random salt every entry starts with.
const SALT: usize = 32;
/// The bytes of the master key.
pub const MASTER: usize = 32;
/// Mixed into every key the vault derives, so a change of layout cannot read an older one.
const VERSION: &[u8] = b"wisp vault 1";
/// What one track may take of the whole vault at most, as a fraction of the limit.
const SHARE: u64 = 4;
/// How long a half-written entry left by a crash stays before a sweep removes it.
const ABANDONED: Duration = Duration::from_secs(24 * 60 * 60);
/// How long a lookup made before the vault has its key waits for it. Reading the keychain takes
/// milliseconds, so this only runs out while the keychain is prompting or stuck.
const UNLOCK_WAIT: Duration = Duration::from_secs(5);

/// Tells apart the temporary files of two recordings running at once in one process.
static WRITES: AtomicU64 = AtomicU64::new(0);

/// Answers the facts a provider keeps beside a track once the whole track has arrived, or
/// `None` when the provider cannot vouch that it has.
type Describe = Box<dyn FnOnce() -> Option<Vec<u8>> + Send>;
/// Empties a player's own folder through the player, or answers false once the player is gone.

/// What a sealed block is, which its nonce carries so one cannot stand in for another.
#[derive(Clone, Copy)]
enum Block {
    Chunk = 0,
    Last = 1,
    Facts = 2,
}

/// Every kept track on this machine, under one master key.
///
/// Tracks live in the cache, which holds what has played within one size limit and gives up the
/// least recently played tracks to stay under it.
///
/// The vault exists before its key does. Until [`unlock`](Self::unlock) or
/// [`stay_locked`](Self::stay_locked) runs, nothing is kept, and a lookup from an engine waits up
/// to [`UNLOCK_WAIT`] for the key rather than missing a track that is there.
pub struct Vault {
    root: PathBuf,
    /// The most the cache may hold, in bytes.
    limit: AtomicU64,
    /// Whether tracks are kept in the cache.
    caching: AtomicBool,
    keys: OnceLock<hkdf::Prk>,
    /// Turns true once the vault has its key or will not get one this session.
    settled: watch::Sender<bool>,
    /// What the cache takes on disk, once a sweep has counted it.
    used: Mutex<Option<u64>>,
}

impl fmt::Debug for Vault {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Vault")
            .field("root", &self.root)
            .field("limit", &self.limit())
            .field("unlocked", &self.keys.get().is_some())
            .finish()
    }
}

impl Vault {
    /// A vault that caches at most `limit` bytes in `root`.
    pub fn new(root: PathBuf, limit: u64) -> Self {
        Self {
            root,
            limit: AtomicU64::new(limit),
            caching: AtomicBool::new(true),
            keys: OnceLock::new(),
            settled: watch::Sender::new(false),
            used: Mutex::new(None),
        }
    }

    /// A vault in Wisp's cache folder, `$XDG_CACHE_HOME/wisp/vault`.
    pub fn standard(limit: u64) -> Self {
        Self::new(standard_root(), limit)
    }

    /// A vault whose cache sits under `root` when the listener chose one, or in Wisp's own
    /// folder otherwise.
    pub fn with_cache(root: Option<PathBuf>, limit: u64) -> Self {
        Self::new(root.unwrap_or_else(standard_root), limit)
    }

    /// The folder the cache lives in, whether the listener chose it or not.
    pub fn cache_root(&self) -> &Path {
        &self.root
    }

    /// Changes how much the cache may hold. A lower limit evicts the least recently played
    /// tracks at once, which walks the cache on disk, so call it off the main thread.
    pub fn set_limit(&self, limit: u64) {
        let before = self.limit.swap(limit, Ordering::Relaxed);
        if limit < before {
            self.sweep();
        }
    }

    fn limit(&self) -> u64 {
        self.limit.load(Ordering::Relaxed)
    }

    /// Whether tracks are kept in the cache.
    pub fn set_caching(&self, on: bool) {
        self.caching.store(on, Ordering::Relaxed);
    }

    /// A fresh random master key, for a machine whose keychain has none yet.
    pub fn new_master() -> Result<[u8; MASTER]> {
        let mut master = [0; MASTER];
        SystemRandom::new()
            .fill(&mut master)
            .map_err(|_| anyhow!("cannot draw a random master key"))?;
        Ok(master)
    }

    /// Lets the vault find and keep tracks under `master`. Only the first call counts.
    pub fn unlock(&self, master: &[u8]) {
        let prk = hkdf::Salt::new(hkdf::HKDF_SHA256, VERSION).extract(master);
        if self.keys.set(prk).is_err() {
            log::debug!("vault: already unlocked");
        }
        self.settled.send_replace(true);
    }

    /// Gives up on the key for this session, so lookups stop waiting for it and miss at once.
    pub fn stay_locked(&self) {
        self.settled.send_replace(true);
    }

    /// Whether the vault has its key yet.
    pub fn unlocked(&self) -> bool {
        self.keys.get().is_some()
    }

    /// Whether the key is still being looked for, so which tracks are downloaded cannot be told
    /// yet.
    pub fn unlocking(&self) -> bool {
        !*self.settled.borrow()
    }

    /// The key, waiting up to [`UNLOCK_WAIT`] for it while it is still being looked for.
    async fn ready(&self) -> Option<&hkdf::Prk> {
        if let Some(prk) = self.keys.get() {
            return Some(prk);
        }
        let mut settled = self.settled.subscribe();
        if tokio::time::timeout(UNLOCK_WAIT, settled.wait_for(|done| *done))
            .await
            .is_err()
        {
            log::warn!("vault: still locked after {UNLOCK_WAIT:?}, looking past it");
        }
        self.keys.get()
    }

    /// The part of the vault one provider keeps its tracks in. `provider` names its folder and
    /// goes into every key, so no provider can open another's entries.
    pub fn shelf(self: &Arc<Self>, provider: &'static str) -> Shelf {
        Shelf {
            vault: self.clone(),
            provider,
            scope: String::new(),
        }
    }

    /// Removes every cached track.
    pub fn forget(&self) {
        let mut failed = false;
        for item in fs::read_dir(&self.root).into_iter().flatten().flatten() {
            let path = item.path();
            if let Err(error) = remove(&path) {
                failed = true;
                log::warn!("vault: cannot forget the cached tracks: {error}");
            }
        }
        if !failed {
            log::info!("vault: forgot every cached track");
        }
        *self.used.lock().unwrap_or_else(|held| held.into_inner()) = None;
    }

    /// What the cache takes on disk, in bytes, the cache counting the files players keep for
    /// themselves. It walks the folder, so call it off the main thread.
    pub fn usage(&self) -> u64 {
        footprint(&self.root)
    }

    /// Counts a new cache entry of `size` bytes and evicts the least recently played ones once
    /// the cache holds more than its limit.
    fn settle(&self, size: u64) {
        let over = {
            let mut used = self.used.lock().unwrap_or_else(|held| held.into_inner());
            *used = used.map(|used| used.saturating_add(size));
            used.is_none_or(|used| used > self.limit())
        };
        if over {
            self.sweep();
        }
    }

    /// Drops what a crash left half-written and the least recently played cache entries until
    /// they fit the limit.
    fn sweep(&self) {
        let limit = self.limit();
        let own = evict(&self.root, limit);
        *self.used.lock().unwrap_or_else(|held| held.into_inner()) = Some(own);
    }
}

/// One provider's tracks in the vault.
#[derive(Clone)]
pub struct Shelf {
    vault: Arc<Vault>,
    provider: &'static str,
    /// What else tells two tracks with one id apart, such as the host an id came from.
    scope: String,
}

impl fmt::Debug for Shelf {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Shelf")
            .field("provider", &self.provider)
            .finish()
    }
}

impl Shelf {
    /// The same shelf for ids that are only unique within `scope`.
    pub(crate) fn within(mut self, scope: impl Into<String>) -> Self {
        self.scope = scope.into();
        self
    }

    /// The kept copy of track `id` as `variant` describes it, when there is a whole one. An
    /// entry that fails to open is removed, so a damaged file costs one download and no more.
    ///
    /// `variant` is whatever changes the bytes for the same id, such as the quality asked for.
    /// A lookup made while the vault is still unlocking waits for it, up to [`UNLOCK_WAIT`].
    pub async fn find(&self, id: &str, variant: &str) -> Option<Kept> {
        if !self.vault.caching.load(Ordering::Relaxed) {
            return None;
        }
        let prk = self.vault.ready().await?;
        self.find_in(prk, id, variant)
    }

    /// Wraps `source` so that everything it yields is sealed into the cache as it passes, and
    /// the entry is kept once the source has yielded its last byte. `facts` answers whatever the
    /// provider needs to play the track again without asking the network, which comes back from
    /// [`Kept::facts`]. It runs when the entry is kept, so answering `None` there keeps nothing.
    ///
    /// The source passes through untouched whatever happens to the vault. A disk that refuses
    /// the entry only means the track is not kept.
    pub fn keep<S: Source>(
        &self,
        id: &str,
        variant: &str,
        facts: impl FnOnce() -> Option<Vec<u8>> + Send + 'static,
        source: S,
    ) -> Recording<S> {
        if !self.vault.caching.load(Ordering::Relaxed) {
            return Recording::through(source);
        }
        let pending = match self.begin(id, variant, Box::new(facts)) {
            Ok(pending) => pending,
            Err(error) => {
                log::debug!("vault: not keeping a {} track: {error:#}", self.provider);
                None
            }
        };
        Recording { source, pending }
    }

    fn find_in(&self, prk: &hkdf::Prk, id: &str, variant: &str) -> Option<Kept> {
        let (name, path) = self.spot(prk, id, variant)?;
        let file = match File::open(&path) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return None,
            Err(error) => {
                log::warn!("vault: cannot open a kept track: {error}");
                return None;
            }
        };
        match self.open(prk, &name, &path, file) {
            Ok(kept) => {
                let now = SystemTime::now();
                if let Ok(file) = OpenOptions::new().write(true).open(&path) {
                    file.set_times(FileTimes::new().set_accessed(now).set_modified(now))
                        .ok();
                }
                Some(kept)
            }
            Err(error) => {
                log::warn!(
                    "vault: dropping a damaged {} track: {error:#}",
                    self.provider
                );
                fs::remove_file(&path).ok();
                None
            }
        }
    }

    /// The folder this shelf keeps its entries in.
    fn folder(&self) -> PathBuf {
        self.vault.root.join(self.provider)
    }

    /// The file name and path of a track. The name is a keyed hash that says nothing to anyone
    /// without the key.
    fn spot(&self, prk: &hkdf::Prk, id: &str, variant: &str) -> Option<(String, PathBuf)> {
        let key: hmac::Key = prk
            .expand(&[b"name", self.provider.as_bytes()], hmac::HMAC_SHA256)
            .ok()?
            .into();
        let mut context = hmac::Context::with_key(&key);
        context.update(id.as_bytes());
        context.update(&[0]);
        context.update(self.scope.as_bytes());
        context.update(&[0]);
        context.update(variant.as_bytes());
        let name = hex(&context.sign().as_ref()[..16]);
        let path = self.folder().join(&name);
        Some((name, path))
    }

    /// The key that seals one entry, from the master key, this shelf and the entry's salt.
    fn key(&self, prk: &hkdf::Prk, salt: &[u8]) -> Result<LessSafeKey> {
        let info = [b"track".as_slice(), self.provider.as_bytes(), salt];
        let okm = prk
            .expand(&info, &CHACHA20_POLY1305)
            .map_err(|_| anyhow!("cannot derive a track key"))?;
        Ok(LessSafeKey::new(UnboundKey::from(okm)))
    }

    /// Opens a temporary file for a new cache entry, or answers `None` while the vault is locked.
    fn begin(&self, id: &str, variant: &str, facts: Describe) -> Result<Option<Pending>> {
        let Some(prk) = self.vault.keys.get() else {
            return Ok(None);
        };
        let (name, path) = self
            .spot(prk, id, variant)
            .context("cannot name the entry")?;
        let folder = self.folder();
        create_private(&self.vault.root)?;
        create_private(&folder)?;
        let mut salt = [0; SALT];
        SystemRandom::new()
            .fill(&mut salt)
            .map_err(|_| anyhow!("cannot draw a salt"))?;
        let key = self.key(prk, &salt)?;
        let temporary = folder.join(format!(
            ".{name}.{}.{}",
            std::process::id(),
            WRITES.fetch_add(1, Ordering::Relaxed)
        ));
        let mut file = private_file(&temporary)?;
        file.write_all(&salt).context("cannot start the entry")?;
        Ok(Some(Pending {
            cache: self.vault.clone(),
            path,
            temporary,
            name,
            file,
            key,
            facts: Some(facts),
            buffer: Vec::with_capacity(CHUNK + TAG),
            index: 0,
            written: SALT as u64,
            ceiling: self.vault.limit() / SHARE,
        }))
    }

    /// Reads the salt and the facts of an entry, and leaves a source at its first chunk.
    fn open(&self, prk: &hkdf::Prk, name: &str, path: &Path, mut file: File) -> Result<Kept> {
        let size = file.metadata()?.len();
        let mut salt = [0; SALT];
        file.read_exact(&mut salt).context("cannot read the salt")?;
        let key = self.key(prk, &salt)?;
        if size < (SALT + TAG + 4) as u64 {
            bail!("the entry is too short");
        }
        let mut length = [0; 4];
        file.seek(SeekFrom::End(-4))?;
        file.read_exact(&mut length)?;
        let sealed = u64::from(u32::from_le_bytes(length));
        let facts_at = size
            .checked_sub(4 + sealed)
            .filter(|at| *at >= SALT as u64)
            .context("the entry's facts run past its start")?;
        let mut facts = vec![0; sealed as usize];
        file.seek(SeekFrom::Start(facts_at))?;
        file.read_exact(&mut facts)?;
        let opened = open(&key, name, Block::Facts, 0, &mut facts)?;
        let (length, facts) = opened
            .split_first_chunk::<8>()
            .context("the entry's facts have no length")?;
        let length = u64::from_le_bytes(*length);
        let facts = facts.to_vec();
        file.seek(SeekFrom::Start(SALT as u64))?;
        Ok(Kept {
            facts,
            length,
            source: Stored {
                file,
                key,
                name: name.to_owned(),
                path: path.to_owned(),
                index: 0,
                left: facts_at - SALT as u64,
                buffer: Vec::with_capacity(CHUNK + TAG),
            },
        })
    }
}

/// A kept track, ready to be played again.
pub struct Kept {
    /// What the provider stored beside the track when it was kept.
    pub facts: Vec<u8>,
    /// The length of the track as the provider sent it.
    pub length: u64,
    /// The track, chunk by chunk, as the provider sent it.
    pub source: Stored,
}

/// The bytes of a kept track, read a chunk at a time as the stream asks for them. A sealed
/// chunk that fails to open removes the entry, so the next play downloads the track again.
pub struct Stored {
    file: File,
    /// What opens each chunk.
    key: LessSafeKey,
    name: String,
    /// Where the entry lies, so a chunk that fails to open can take the entry with it.
    path: PathBuf,
    index: u64,
    /// The stored bytes still to read.
    left: u64,
    buffer: Vec<u8>,
}

impl Source for Stored {
    async fn chunk(&mut self) -> Result<Option<Bytes>> {
        let chunk = self.open_next();
        if let Err(error) = &chunk {
            log::warn!("vault: cannot read a kept track: {error:#}");
            fs::remove_file(&self.path).ok();
        }
        chunk
    }
}

impl Stored {
    /// Reads and opens the next chunk, or answers `None` once there are none left.
    fn open_next(&mut self) -> Result<Option<Bytes>> {
        if self.left == 0 {
            return Ok(None);
        }
        let take = self.left.min((CHUNK + TAG) as u64) as usize;
        self.buffer.resize(take, 0);
        self.file
            .read_exact(&mut self.buffer)
            .context("cannot read a kept track")?;
        self.left -= take as u64;
        let block = match self.left {
            0 => Block::Last,
            _ => Block::Chunk,
        };
        let opened = open(&self.key, &self.name, block, self.index, &mut self.buffer)?;
        self.index += 1;
        Ok(Some(Bytes::copy_from_slice(opened)))
    }
}

/// A source whose bytes are sealed into the vault as they pass through.
pub struct Recording<S> {
    source: S,
    /// The entry being written, until it is kept or given up on.
    pending: Option<Pending>,
}

impl<S> Recording<S> {
    /// A source that passes through without keeping anything, for a provider with no shelf.
    pub fn through(source: S) -> Self {
        Self {
            source,
            pending: None,
        }
    }
}

impl<S: Source> Source for Recording<S> {
    async fn chunk(&mut self) -> Result<Option<Bytes>> {
        let chunk = self.source.chunk().await?;
        if let Some(mut pending) = self.pending.take() {
            let written = match &chunk {
                Some(bytes) => pending.write(bytes).map(|()| Some(pending)),
                None => pending.commit().map(|()| None),
            };
            match written {
                Ok(pending) => self.pending = pending,
                Err(error) => log::warn!("vault: cannot keep the track: {error:#}"),
            }
        }
        Ok(chunk)
    }
}

/// An entry being written. Dropping it before [`commit`](Self::commit) removes the file, so
/// a skipped or broken track leaves nothing behind.
struct Pending {
    /// The vault whose cache the entry counts against.
    cache: Arc<Vault>,
    /// Where the sealed entry goes.
    path: PathBuf,
    temporary: PathBuf,
    name: String,
    file: File,
    /// Seals every chunk and the facts.
    key: LessSafeKey,
    /// Answers the provider's facts once the whole track is in. Taken when the entry is kept.
    facts: Option<Describe>,
    /// What has arrived since the last sealed chunk, and the room to seal it in.
    buffer: Vec<u8>,
    index: u64,
    /// The bytes of the track the provider has sent so far, salt included.
    written: u64,
    /// The most one entry may take before it is given up on.
    ceiling: u64,
}

impl Pending {
    /// Seals every whole chunk that has built up. One is always held back, so the last chunk
    /// sealed is never empty unless the whole track is.
    fn write(&mut self, bytes: &[u8]) -> Result<()> {
        self.written += bytes.len() as u64;
        if self.written > self.ceiling {
            bail!("the track is larger than one entry may be");
        }
        let mut bytes = bytes;
        while !bytes.is_empty() {
            let room = CHUNK + 1 - self.buffer.len().min(CHUNK + 1);
            let (head, rest) = bytes.split_at(room.min(bytes.len()));
            self.buffer.extend_from_slice(head);
            bytes = rest;
            if self.buffer.len() > CHUNK {
                let spill = self.buffer.split_off(CHUNK);
                self.seal(Block::Chunk)?;
                self.buffer = spill;
            }
        }
        Ok(())
    }

    /// Seals what is held as the last chunk, then the facts, and moves the entry into place.
    fn commit(mut self) -> Result<()> {
        let told = self
            .facts
            .take()
            .and_then(|facts| facts())
            .context("the provider cannot vouch for the whole track")?;
        let length = self.written - SALT as u64;
        self.seal(Block::Last)?;
        let mut facts = Vec::with_capacity(8 + told.len() + TAG);
        facts.extend_from_slice(&length.to_le_bytes());
        facts.extend_from_slice(&told);
        self.key
            .seal_in_place_append_tag(
                nonce(Block::Facts, 0),
                Aad::from(self.name.as_bytes()),
                &mut facts,
            )
            .map_err(|_| anyhow!("cannot seal the facts"))?;
        let sealed = u32::try_from(facts.len()).context("the facts are too long")?;
        self.file.write_all(&facts)?;
        self.file.write_all(&sealed.to_le_bytes())?;
        self.file.flush()?;
        let size = self.file.metadata()?.len();
        fs::rename(&self.temporary, &self.path).context("cannot move the entry into place")?;
        log::debug!(
            "vault: kept a track, {:.1} MiB",
            size as f64 / (1024.0 * 1024.0)
        );
        self.cache.settle(size);
        Ok(())
    }

    fn seal(&mut self, block: Block) -> Result<()> {
        self.key
            .seal_in_place_append_tag(
                nonce(block, self.index),
                Aad::from(self.name.as_bytes()),
                &mut self.buffer,
            )
            .map_err(|_| anyhow!("cannot seal a chunk"))?;
        self.file
            .write_all(&self.buffer)
            .context("cannot write a chunk")?;
        self.buffer.clear();
        self.index += 1;
        Ok(())
    }
}

impl Drop for Pending {
    fn drop(&mut self) {
        // Once the entry is in place the temporary name is gone and this finds nothing.
        fs::remove_file(&self.temporary).ok();
    }
}

/// The nonce of one sealed block. Every entry has its own key, so the index alone keeps a
/// nonce from repeating.
fn nonce(block: Block, index: u64) -> Nonce {
    let mut nonce = [0; NONCE_LEN];
    nonce[..8].copy_from_slice(&index.to_be_bytes());
    nonce[8] = block as u8;
    Nonce::assume_unique_for_key(nonce)
}

/// Opens one sealed block in place and answers the plain bytes inside it.
fn open<'a>(
    key: &LessSafeKey,
    name: &str,
    block: Block,
    index: u64,
    sealed: &'a mut [u8],
) -> Result<&'a mut [u8]> {
    key.open_in_place(nonce(block, index), Aad::from(name.as_bytes()), sealed)
        .map_err(|_| anyhow!("a sealed block does not open"))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Creates `path` readable by the owner alone, or makes it so when another program created it
/// open to everyone.
fn create_private(path: &Path) -> Result<()> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
    builder
        .create(path)
        .with_context(|| format!("cannot create {}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mode = fs::metadata(path)
            .with_context(|| format!("cannot read {}", path.display()))?
            .permissions()
            .mode();
        if mode & 0o077 != 0 {
            fs::set_permissions(path, fs::Permissions::from_mode(0o700))
                .with_context(|| format!("cannot make {} private", path.display()))?;
        }
    }
    Ok(())
}

/// Removes a file or a whole folder.
fn remove(path: &Path) -> io::Result<()> {
    match fs::symlink_metadata(path)?.is_dir() {
        true => fs::remove_dir_all(path),
        false => fs::remove_file(path),
    }
}

/// Empties the folder of players keeping their own files. A running player clears its own
/// folder through `players`, and the folder of any other is removed.
/// Evicts the least recently played entries under `root` until they take `room` bytes or
/// fewer, and answers what they take after. Half-written entries a crash left are dropped on
/// the way.
fn evict(root: &Path, room: u64) -> u64 {
    let mut entries = abandoned(root);
    entries.sort_unstable_by_key(|(_, used, _)| *used);
    let mut total = entries.iter().map(|(_, _, size)| size).sum::<u64>();
    for (path, _, size) in entries {
        if total <= room {
            break;
        }
        if fs::remove_file(&path).is_ok() {
            total = total.saturating_sub(size);
        }
    }
    total
}

/// A new file readable and writable by the owner alone.
fn private_file(path: &Path) -> Result<File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    options
        .open(path)
        .with_context(|| format!("cannot create {}", path.display()))
}

/// Wisp's vault folder, `$XDG_CACHE_HOME/wisp/vault`.
fn standard_root() -> PathBuf {
    crate::credentials::root().join("vault")
}

/// What everything under `path` takes on disk, in bytes. A folder that cannot be read counts as
/// empty.
fn footprint(path: &Path) -> u64 {
    fs::read_dir(path)
        .into_iter()
        .flatten()
        .flatten()
        .map(|item| match item.file_type() {
            Ok(kind) if kind.is_dir() => footprint(&item.path()),
            Ok(_) => item.metadata().map_or(0, |metadata| metadata.len()),
            Err(_) => 0,
        })
        .sum()
}

/// Removes what a crash left half-written under `root` a while ago, and answers every whole
/// entry there with when it last played and its size.
fn abandoned(root: &Path) -> Vec<(PathBuf, SystemTime, u64)> {
    let now = SystemTime::now();
    let mut entries = Vec::new();
    for shelf in fs::read_dir(root).into_iter().flatten().flatten() {
        for item in fs::read_dir(shelf.path()).into_iter().flatten().flatten() {
            let Ok(metadata) = item.metadata() else {
                continue;
            };
            if metadata.is_dir() {
                continue;
            }
            let used = metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH);
            let partial = item.file_name().to_string_lossy().starts_with('.');
            if partial {
                if now.duration_since(used).is_ok_and(|age| age > ABANDONED) {
                    fs::remove_file(item.path()).ok();
                }
                continue;
            }
            entries.push((item.path(), used, metadata.len()));
        }
    }
    entries
}
