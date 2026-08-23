//! Exact retained-object admission for the bundled `SQLite` VFS.
//!
//! A pathname check before `sqlite3_open_v2` is not an object binding: on
//! Unix another process with the same filesystem authority can rename the
//! admitted file and install a different inode before the VFS opens it. This
//! crate wraps the frozen bundled VFS and verifies the native object returned
//! by its `MAIN_DB` `xOpen` callback before that callback returns to `SQLite`.
//! The wrapper delegates every other operation to the stock VFS, preserving
//! its ordinary WAL, shared-memory, locking, and durability behavior.
//!
//! Product crates keep `unsafe_code = "forbid"`. All raw `SQLite` and OS
//! inspection is isolated below and the public API exposes neither raw file
//! descriptors nor native handles.

#![deny(unsafe_code)]

#[allow(unsafe_code)]
mod implementation {
    use std::{
        collections::HashMap,
        ffi::{CStr, CString, c_int},
        fmt,
        fs::File,
        io,
        ops::{Deref, DerefMut},
        path::Path,
        ptr::{self, NonNull},
        sync::{
            Arc, Mutex, OnceLock,
            atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering},
        },
    };

    use rusqlite::{Connection, OpenFlags, ffi};

    const SQLITE_VERSION: &str = "3.53.4";
    const SQLITE_SOURCE_ID: &str =
        "2026-07-24 19:02:57 bf7c7f30031888f4e796e429ab3978879485813aaca6f641c7b33e4e09459bcc";
    static NEXT_VFS_ID: AtomicU64 = AtomicU64::new(0);

    /// A closed failure from exact-object VFS admission.
    #[derive(Debug)]
    pub enum ExactSqliteOpenError {
        /// The retained authority is not a regular file or cannot be queried.
        RetainedFile(io::Error),
        /// The requested database pathname is not an absolute ordinary path.
        InvalidPath,
        /// The requested flags do not describe one exact ordinary disk file.
        InvalidOpenFlags,
        /// The linked engine does not match the reviewed VFS layout.
        EngineIdentity {
            actual_version: String,
            actual_source_id: String,
        },
        /// The reviewed stock platform VFS is unavailable.
        StockVfsUnavailable,
        /// Registration of the private wrapper VFS failed.
        VfsRegistration(c_int),
        /// An exact open was re-entered on the same thread before admission ended.
        AdmissionAlreadyActive,
        /// The stock VFS opened an object other than the retained authority.
        MainDatabaseIdentityMismatch,
        /// More than one `MAIN_DB` open was attempted through a one-shot VFS.
        AdditionalMainDatabase,
        /// Native identity inspection of the stock VFS file failed.
        NativeIdentity,
        /// `SQLite` returned without opening and binding the main database.
        MainDatabaseNotBound,
        /// The delegated `SQLite` open failed for another reason.
        Sqlite(rusqlite::Error),
    }

    impl fmt::Display for ExactSqliteOpenError {
        fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            match self {
                Self::RetainedFile(error) => write!(formatter, "retained SQLite file: {error}"),
                Self::InvalidPath => {
                    formatter.write_str("exact SQLite open requires an absolute ordinary path")
                }
                Self::InvalidOpenFlags => formatter.write_str(
                    "exact SQLite open requires NOFOLLOW, exactly one access mode, and no URI, memory, create, shared-cache, or full-mutex flags",
                ),
                Self::EngineIdentity {
                    actual_version,
                    actual_source_id,
                } => write!(
                    formatter,
                    "bundled SQLite identity mismatch: version {actual_version}, source {actual_source_id}"
                ),
                Self::StockVfsUnavailable => {
                    formatter.write_str("reviewed stock SQLite VFS is unavailable")
                }
                Self::VfsRegistration(code) => {
                    write!(
                        formatter,
                        "private SQLite VFS registration failed with {code}"
                    )
                }
                Self::AdmissionAlreadyActive => formatter
                    .write_str("another exact SQLite admission is active on this thread"),
                Self::MainDatabaseIdentityMismatch => {
                    formatter.write_str("SQLite MAIN_DB differs from the retained file authority")
                }
                Self::AdditionalMainDatabase => {
                    formatter.write_str("the exact one-shot VFS rejected another MAIN_DB open")
                }
                Self::NativeIdentity => {
                    formatter.write_str("SQLite MAIN_DB native identity could not be established")
                }
                Self::MainDatabaseNotBound => {
                    formatter.write_str("SQLite returned without an exact MAIN_DB binding")
                }
                Self::Sqlite(error) => write!(formatter, "SQLite open: {error}"),
            }
        }
    }

    impl std::error::Error for ExactSqliteOpenError {
        fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
            match self {
                Self::RetainedFile(error) => Some(error),
                Self::Sqlite(error) => Some(error),
                _ => None,
            }
        }
    }

    /// A `SQLite` connection whose stock VFS `MAIN_DB` object was matched to a
    /// retained caller-owned file before `xOpen` returned.
    ///
    /// The connection field is deliberately declared before the registered
    /// VFS. Rust drops fields in declaration order, so `SQLite` closes every
    /// main/WAL/SHM object before the private VFS is unregistered and its
    /// retained file authority is released.
    pub struct ExactSqliteConnection {
        connection: Connection,
        // This field exists for its drop/lifetime effect; tests also inspect
        // its private name to prove registration lifetime and uniqueness.
        #[cfg_attr(not(test), allow(dead_code))]
        registered_vfs: RegisteredExactVfs,
    }

    impl Deref for ExactSqliteConnection {
        type Target = Connection;

        fn deref(&self) -> &Self::Target {
            &self.connection
        }
    }

    impl DerefMut for ExactSqliteConnection {
        fn deref_mut(&mut self) -> &mut Self::Target {
            &mut self.connection
        }
    }

    impl AsRef<Connection> for ExactSqliteConnection {
        fn as_ref(&self) -> &Connection {
            &self.connection
        }
    }

    impl AsMut<Connection> for ExactSqliteConnection {
        fn as_mut(&mut self) -> &mut Connection {
            &mut self.connection
        }
    }

    /// Opens `path` through the reviewed stock VFS and accepts the connection
    /// only when its actual `MAIN_DB` object is `retained`.
    ///
    /// `retained` remains open for the complete connection lifetime. The
    /// caller must precreate the file. `flags` must select exactly one of
    /// read-only or read-write, must contain `SQLITE_OPEN_NOFOLLOW`, and must
    /// not enable URI, memory, creation, shared-cache, or full-mutex behavior.
    /// `SQLITE_OPEN_NO_MUTEX` is deliberately accepted: `rusqlite::Connection`
    /// is `Send` but not `Sync`, so its ordinary static access discipline is
    /// the reviewed mutex contract. Keeping creation and filename
    /// interpretation outside `SQLite` ensures a failed identity comparison
    /// cannot select or create a different database as a side effect.
    ///
    /// # Errors
    ///
    /// Returns a closed error when the engine/VFS is not the reviewed bundled
    /// implementation, the retained object is unsafe, the actual main file is
    /// different, or the delegated `SQLite` open fails.
    pub fn open_exact(
        retained: File,
        path: &Path,
        flags: OpenFlags,
    ) -> Result<ExactSqliteConnection, ExactSqliteOpenError> {
        open_exact_inner(
            retained,
            path,
            flags,
            test_before_owner_open_hook_none(),
            test_hook_none(),
        )
    }

    #[cfg(test)]
    type BeforeOwnerOpenHook = Box<dyn FnOnce(&CStr) -> io::Result<()> + Send>;

    #[cfg(not(test))]
    type BeforeOwnerOpenHook = ();

    #[cfg(test)]
    type AfterBaseOpenHook = Box<dyn FnOnce() -> io::Result<()> + Send>;

    #[cfg(not(test))]
    type AfterBaseOpenHook = ();

    #[cfg(test)]
    fn test_before_owner_open_hook_none() -> Option<BeforeOwnerOpenHook> {
        None
    }

    #[cfg(not(test))]
    const fn test_before_owner_open_hook_none() -> Option<BeforeOwnerOpenHook> {
        None
    }

    #[cfg(test)]
    fn test_hook_none() -> Option<AfterBaseOpenHook> {
        None
    }

    #[cfg(not(test))]
    const fn test_hook_none() -> Option<AfterBaseOpenHook> {
        None
    }

    fn open_exact_inner(
        retained: File,
        path: &Path,
        flags: OpenFlags,
        #[cfg_attr(not(test), allow(unused_variables))] before_owner_open: Option<
            BeforeOwnerOpenHook,
        >,
        after_base_open: Option<AfterBaseOpenHook>,
    ) -> Result<ExactSqliteConnection, ExactSqliteOpenError> {
        if !path.is_absolute() || path.as_os_str().is_empty() {
            return Err(ExactSqliteOpenError::InvalidPath);
        }
        validate_open_flags(flags)?;
        let expected =
            platform::retained_identity(&retained).map_err(ExactSqliteOpenError::RetainedFile)?;
        require_engine_identity()?;
        let registered_vfs =
            RegisteredExactVfs::register(retained, expected, before_owner_open, after_base_open)?;
        let name = registered_vfs
            .name_cstr()
            .ok_or(ExactSqliteOpenError::StockVfsUnavailable)?;
        let connection = match Connection::open_with_flags_and_vfs(path, flags, name) {
            Ok(connection) => connection,
            Err(error) => {
                return Err(match registered_vfs.failure() {
                    CallbackFailure::IdentityMismatch => {
                        ExactSqliteOpenError::MainDatabaseIdentityMismatch
                    }
                    CallbackFailure::AdditionalMain => ExactSqliteOpenError::AdditionalMainDatabase,
                    CallbackFailure::NativeIdentity => ExactSqliteOpenError::NativeIdentity,
                    CallbackFailure::None => ExactSqliteOpenError::Sqlite(error),
                });
            }
        };
        let failure = registered_vfs.failure();
        if failure != CallbackFailure::None {
            drop(connection);
            return Err(match failure {
                CallbackFailure::IdentityMismatch => {
                    ExactSqliteOpenError::MainDatabaseIdentityMismatch
                }
                CallbackFailure::AdditionalMain => ExactSqliteOpenError::AdditionalMainDatabase,
                CallbackFailure::NativeIdentity | CallbackFailure::None => {
                    ExactSqliteOpenError::NativeIdentity
                }
            });
        }
        if !registered_vfs.main_bound() {
            drop(connection);
            return Err(ExactSqliteOpenError::MainDatabaseNotBound);
        }
        Ok(ExactSqliteConnection {
            connection,
            registered_vfs,
        })
    }

    fn validate_open_flags(flags: OpenFlags) -> Result<(), ExactSqliteOpenError> {
        let access = flags & (OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_READ_WRITE);
        let one_access_mode = access == OpenFlags::SQLITE_OPEN_READ_ONLY
            || access == OpenFlags::SQLITE_OPEN_READ_WRITE;
        let forbidden = OpenFlags::SQLITE_OPEN_CREATE
            | OpenFlags::SQLITE_OPEN_URI
            | OpenFlags::SQLITE_OPEN_MEMORY
            | OpenFlags::SQLITE_OPEN_FULL_MUTEX
            | OpenFlags::SQLITE_OPEN_SHARED_CACHE;
        let allowed = OpenFlags::SQLITE_OPEN_READ_ONLY
            | OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_PRIVATE_CACHE
            | OpenFlags::SQLITE_OPEN_NOFOLLOW
            | OpenFlags::SQLITE_OPEN_EXRESCODE;
        if !one_access_mode
            || !flags.contains(OpenFlags::SQLITE_OPEN_NOFOLLOW)
            || flags.intersects(forbidden)
            || !flags.difference(allowed).is_empty()
        {
            return Err(ExactSqliteOpenError::InvalidOpenFlags);
        }
        Ok(())
    }

    fn require_engine_identity() -> Result<(), ExactSqliteOpenError> {
        // SAFETY: these SQLite process-global identity functions return static
        // NUL-terminated strings for the linked library. Null is handled
        // without dereferencing it.
        let (version, source_id) = unsafe {
            (
                sqlite_static_text(ffi::sqlite3_libversion()),
                sqlite_static_text(ffi::sqlite3_sourceid()),
            )
        };
        let actual_version = version.unwrap_or_default().to_owned();
        let actual_source_id = source_id.unwrap_or_default().to_owned();
        if actual_version != SQLITE_VERSION || actual_source_id != SQLITE_SOURCE_ID {
            return Err(ExactSqliteOpenError::EngineIdentity {
                actual_version,
                actual_source_id,
            });
        }
        Ok(())
    }

    unsafe fn sqlite_static_text(pointer: *const std::ffi::c_char) -> Option<&'static str> {
        if pointer.is_null() {
            return None;
        }
        // SAFETY: the caller supplies one of SQLite's static identity strings,
        // checked non-null above. SQLite guarantees NUL termination and static
        // lifetime for both identity APIs.
        unsafe { CStr::from_ptr(pointer) }.to_str().ok()
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    #[repr(u8)]
    enum CallbackFailure {
        None = 0,
        IdentityMismatch = 1,
        AdditionalMain = 2,
        NativeIdentity = 3,
    }

    impl CallbackFailure {
        fn from_byte(value: u8) -> Self {
            match value {
                1 => Self::IdentityMismatch,
                2 => Self::AdditionalMain,
                3 => Self::NativeIdentity,
                _ => Self::None,
            }
        }
    }

    struct ExactVfsState {
        base_address: usize,
        base_open: VfsOpen,
        expected: platform::FileIdentity,
        _retained: File,
        main_attempted: AtomicBool,
        main_bound: AtomicBool,
        failure: AtomicU8,
        name: CString,
        #[cfg(test)]
        after_base_open: std::sync::Mutex<Option<AfterBaseOpenHook>>,
    }

    type VfsOpen = unsafe extern "C" fn(
        *mut ffi::sqlite3_vfs,
        ffi::sqlite3_filename,
        *mut ffi::sqlite3_file,
        c_int,
        *mut c_int,
    ) -> c_int;

    #[derive(Default)]
    struct ExactVfsRegistry {
        states: HashMap<usize, Arc<ExactVfsState>>,
    }

    fn exact_vfs_registry() -> &'static Mutex<ExactVfsRegistry> {
        static REGISTRY: OnceLock<Mutex<ExactVfsRegistry>> = OnceLock::new();
        REGISTRY.get_or_init(|| Mutex::new(ExactVfsRegistry::default()))
    }

    struct RegisteredExactVfs {
        vfs: Option<NonNull<ffi::sqlite3_vfs>>,
        state: Option<Arc<ExactVfsState>>,
    }

    // SAFETY: moving RegisteredExactVfs moves only the owning Box, never the
    // stable sqlite3_vfs allocation registered with SQLite. Rust never forms a
    // reference to that allocation while it is registered, because SQLite may
    // mutate its pNext list field. Callback state is kept in a separate Arc and
    // contains only Send + Sync fields. rusqlite makes Connection Send but not
    // Sync; the connection is declared before this guard and therefore closes
    // before unregistration on whichever single thread owns it.
    unsafe impl Send for RegisteredExactVfs {}

    impl RegisteredExactVfs {
        fn register(
            retained: File,
            expected: platform::FileIdentity,
            #[cfg_attr(not(test), allow(unused_variables))] before_owner_open: Option<
                BeforeOwnerOpenHook,
            >,
            #[cfg_attr(not(test), allow(unused_variables))] after_base_open: Option<
                AfterBaseOpenHook,
            >,
        ) -> Result<Self, ExactSqliteOpenError> {
            // SAFETY: sqlite3_initialize is process-global and idempotent. It
            // is required before consulting the registered VFS list.
            let initialize = unsafe { ffi::sqlite3_initialize() };
            if initialize != ffi::SQLITE_OK {
                return Err(ExactSqliteOpenError::VfsRegistration(initialize));
            }
            // SQLite mutates VFS pNext fields during registration. Our one
            // process-global lock serializes every find/copy/register/unregister
            // performed by this module, and the copy below deliberately never
            // reads pNext. Code that calls SQLite's raw VFS registry API outside
            // this module remains outside this safe wrapper's trust boundary.
            let mut registry = exact_vfs_registry()
                .lock()
                .map_err(|_| ExactSqliteOpenError::StockVfsUnavailable)?;
            // SAFETY: platform::stock_vfs_name is a static NUL-terminated
            // string. The returned VFS is owned by SQLite for process life.
            let base = unsafe { ffi::sqlite3_vfs_find(platform::stock_vfs_name().as_ptr()) };
            if base.is_null() {
                return Err(ExactSqliteOpenError::StockVfsUnavailable);
            }
            #[cfg(unix)]
            {
                let required = c_int::try_from(std::mem::size_of::<platform::UnixFilePrefix>())
                    .map_err(|_| ExactSqliteOpenError::StockVfsUnavailable)?;
                // SAFETY: base was checked non-null and SQLite owns it for the
                // process lifetime. szOsFile is immutable after registration;
                // this raw field read does not alias SQLite's mutable pNext.
                if unsafe { ptr::addr_of!((*base).szOsFile).read() } < required {
                    return Err(ExactSqliteOpenError::StockVfsUnavailable);
                }
            }
            let identifier = NEXT_VFS_ID
                .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
                    value.checked_add(1)
                })
                .map_err(|_| ExactSqliteOpenError::StockVfsUnavailable)?;
            let name = CString::new(format!(
                "worldstream-exact-v1-{}-{identifier}",
                std::process::id()
            ))
            .map_err(|_| ExactSqliteOpenError::StockVfsUnavailable)?;
            // SAFETY: base is the reviewed process-lifetime stock VFS. The
            // helper raw-reads each immutable public field separately and sets
            // pNext explicitly, avoiding a shared reference or structural read
            // that would alias SQLite's mutable registry link.
            let stock_fields = unsafe { copy_stock_vfs_without_registry_link(base) };
            let base_open = stock_fields
                .xOpen
                .ok_or(ExactSqliteOpenError::StockVfsUnavailable)?;
            let state = Arc::new(ExactVfsState {
                base_address: base.cast::<()>() as usize,
                base_open,
                expected,
                _retained: retained,
                main_attempted: AtomicBool::new(false),
                main_bound: AtomicBool::new(false),
                failure: AtomicU8::new(CallbackFailure::None as u8),
                name,
                #[cfg(test)]
                after_base_open: std::sync::Mutex::new(after_base_open),
            });
            #[cfg(test)]
            if let Some(hook) = before_owner_open {
                hook(state.name.as_c_str()).map_err(ExactSqliteOpenError::RetainedFile)?;
            }
            let mut private_vfs = stock_fields;
            private_vfs.pNext = ptr::null_mut();
            private_vfs.zName = state.name.as_ptr();
            private_vfs.xOpen = Some(exact_x_open);
            let wrapper = Box::into_raw(Box::new(private_vfs));
            // SAFETY: Box::into_raw never returns null.
            let wrapper_pointer = unsafe { NonNull::new_unchecked(wrapper) };
            let key = wrapper_pointer.as_ptr() as usize;
            if registry.states.insert(key, Arc::clone(&state)).is_some() {
                // SAFETY: registration was not attempted and wrapper is still
                // uniquely owned by this function.
                drop(unsafe { Box::from_raw(wrapper_pointer.as_ptr()) });
                return Err(ExactSqliteOpenError::StockVfsUnavailable);
            }
            // SAFETY: wrapper is a stable heap allocation represented in the
            // callback map. makeDflt=0 leaves the stock default unchanged.
            let registration = unsafe { ffi::sqlite3_vfs_register(wrapper_pointer.as_ptr(), 0) };
            if registration != ffi::SQLITE_OK {
                registry.states.remove(&key);
                // SAFETY: failed registration leaves this allocation solely
                // owned by the function.
                drop(unsafe { Box::from_raw(wrapper_pointer.as_ptr()) });
                return Err(ExactSqliteOpenError::VfsRegistration(registration));
            }
            drop(registry);
            Ok(Self {
                vfs: Some(wrapper_pointer),
                state: Some(state),
            })
        }

        fn state(&self) -> Option<&ExactVfsState> {
            self.state.as_deref()
        }

        fn name_cstr(&self) -> Option<&CStr> {
            self.state().map(|state| state.name.as_c_str())
        }

        fn failure(&self) -> CallbackFailure {
            self.state()
                .map_or(CallbackFailure::NativeIdentity, |state| {
                    CallbackFailure::from_byte(state.failure.load(Ordering::Acquire))
                })
        }

        fn main_bound(&self) -> bool {
            self.state()
                .is_some_and(|state| state.main_bound.load(Ordering::Acquire))
        }
    }

    impl Drop for RegisteredExactVfs {
        fn drop(&mut self) {
            let Some(vfs) = self.vfs.take() else {
                return;
            };
            let Some(state) = self.state.take() else {
                // The VFS is still registered, so its allocation cannot be
                // freed safely. This branch is unreachable through the private
                // constructors but fails closed if invariants are violated.
                return;
            };
            let Ok(mut registry) = exact_vfs_registry().lock() else {
                // A poisoned registry cannot prove that callbacks are gone.
                // Its map still owns an Arc; retain both authorities forever.
                std::mem::forget(state);
                return;
            };
            // SAFETY: vfs remains registered at this exact raw allocation. The
            // enclosing ExactSqliteConnection closes first, and the registry
            // mutex serializes this mutation with every wrapper registry call.
            let result = unsafe { ffi::sqlite3_vfs_unregister(vfs.as_ptr()) };
            if result != ffi::SQLITE_OK {
                // Failing closed against a use-after-free is more important
                // than reclaiming one tiny process-local registration on an
                // impossible SQLite unregistration failure.
                std::mem::forget(state);
                return;
            }
            registry.states.remove(&(vfs.as_ptr() as usize));
            drop(registry);
            // SAFETY: successful unregistration and map removal prove SQLite
            // can no longer reach the uniquely owned VFS allocation.
            drop(unsafe { Box::from_raw(vfs.as_ptr()) });
        }
    }

    unsafe fn copy_stock_vfs_without_registry_link(
        base: *mut ffi::sqlite3_vfs,
    ) -> ffi::sqlite3_vfs {
        // SQLite invokes copied callbacks other than xOpen with the wrapper
        // VFS pointer. That is the documented VFS-subclass contract: the
        // wrapper preserves the stock public configuration and pAppData
        // verbatim. The pinned Unix/Win32 implementations either ignore that
        // pointer or consult only those copied public fields. xOpen is the
        // exception below because native file construction may require the
        // stock VFS identity; exact_x_open therefore passes `base` explicitly.
        macro_rules! read_field {
            ($field:ident) => {{
                // SAFETY: caller guarantees a live reviewed stock VFS. Each
                // listed field is immutable after stock registration and is
                // read through a raw pointer so no shared reference covers the
                // concurrently mutable pNext field.
                unsafe { ptr::addr_of!((*base).$field).read() }
            }};
        }
        ffi::sqlite3_vfs {
            iVersion: read_field!(iVersion),
            szOsFile: read_field!(szOsFile),
            mxPathname: read_field!(mxPathname),
            pNext: ptr::null_mut(),
            zName: ptr::null(),
            pAppData: read_field!(pAppData),
            xOpen: read_field!(xOpen),
            xDelete: read_field!(xDelete),
            xAccess: read_field!(xAccess),
            xFullPathname: read_field!(xFullPathname),
            xDlOpen: read_field!(xDlOpen),
            xDlError: read_field!(xDlError),
            xDlSym: read_field!(xDlSym),
            xDlClose: read_field!(xDlClose),
            xRandomness: read_field!(xRandomness),
            xSleep: read_field!(xSleep),
            xCurrentTime: read_field!(xCurrentTime),
            xGetLastError: read_field!(xGetLastError),
            xCurrentTimeInt64: read_field!(xCurrentTimeInt64),
            xSetSystemCall: read_field!(xSetSystemCall),
            xGetSystemCall: read_field!(xGetSystemCall),
            xNextSystemCall: read_field!(xNextSystemCall),
        }
    }

    unsafe extern "C" fn exact_x_open(
        vfs: *mut ffi::sqlite3_vfs,
        name: ffi::sqlite3_filename,
        file: *mut ffi::sqlite3_file,
        flags: c_int,
        output_flags: *mut c_int,
    ) -> c_int {
        if vfs.is_null() || file.is_null() {
            return ffi::SQLITE_CANTOPEN;
        }
        let state = {
            let Ok(registry) = exact_vfs_registry().lock() else {
                return ffi::SQLITE_CANTOPEN;
            };
            registry.states.get(&(vfs as usize)).cloned()
        };
        let Some(state) = state else {
            return ffi::SQLITE_CANTOPEN;
        };
        let base_open = state.base_open;
        let base = state.base_address as *mut ffi::sqlite3_vfs;
        // SAFETY: all arguments originate from SQLite's xOpen invocation. The
        // stock callback receives its own base VFS pointer, preserving its
        // internal pAppData and file initialization contract.
        let result = unsafe { base_open(base, name, file, flags, output_flags) };
        if result != ffi::SQLITE_OK || flags & ffi::SQLITE_OPEN_MAIN_DB == 0 {
            return result;
        }
        if state.main_attempted.swap(true, Ordering::AcqRel) {
            state
                .failure
                .store(CallbackFailure::AdditionalMain as u8, Ordering::Release);
            // SAFETY: the successful stock xOpen initialized file.
            unsafe { close_rejected_file(file) };
            return ffi::SQLITE_CANTOPEN;
        }
        #[cfg(test)]
        if !run_after_base_open_hook(&state) {
            state
                .failure
                .store(CallbackFailure::NativeIdentity as u8, Ordering::Release);
            // SAFETY: the successful stock xOpen initialized file.
            unsafe { close_rejected_file(file) };
            return ffi::SQLITE_CANTOPEN;
        }
        // SAFETY: the stock xOpen succeeded, so file contains the reviewed
        // platform sqlite3_file implementation until xClose below or later
        // connection teardown.
        match unsafe { platform::sqlite_file_identity(file) } {
            Ok(actual) if actual == state.expected => {
                state.main_bound.store(true, Ordering::Release);
                ffi::SQLITE_OK
            }
            Ok(_) => {
                state
                    .failure
                    .store(CallbackFailure::IdentityMismatch as u8, Ordering::Release);
                // SAFETY: the successful stock xOpen initialized file.
                unsafe { close_rejected_file(file) };
                ffi::SQLITE_CANTOPEN
            }
            Err(()) => {
                state
                    .failure
                    .store(CallbackFailure::NativeIdentity as u8, Ordering::Release);
                // SAFETY: the successful stock xOpen initialized file.
                unsafe { close_rejected_file(file) };
                ffi::SQLITE_CANTOPEN
            }
        }
    }

    #[cfg(test)]
    fn run_after_base_open_hook(state: &ExactVfsState) -> bool {
        let hook = match state.after_base_open.lock() {
            Ok(mut slot) => slot.take(),
            Err(_) => return false,
        };
        let Some(hook) = hook else {
            return true;
        };
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(hook))
            .is_ok_and(|result| result.is_ok())
    }

    unsafe fn close_rejected_file(file: *mut ffi::sqlite3_file) {
        if file.is_null() {
            return;
        }
        // SAFETY: caller guarantees a successful stock xOpen. pMethods is
        // therefore either a valid stock method table or null on a violated
        // VFS contract, which is handled without dereference.
        let methods = unsafe { (*file).pMethods };
        if !methods.is_null() {
            // SAFETY: methods is the live stock table installed in file.
            if let Some(close) = unsafe { (*methods).xClose } {
                // SAFETY: file is the same initialized sqlite3_file passed to
                // xOpen and has not yet been closed.
                let _ = unsafe { close(file) };
            }
        }
        // SQLite requires pMethods=NULL when xOpen reports failure, including
        // when a wrapping VFS rejects a successfully opened lower layer.
        // SAFETY: file points to writable SQLite-owned sqlite3_file storage for
        // the duration of this callback.
        unsafe { (*file).pMethods = ptr::null() };
    }

    #[cfg(unix)]
    mod platform {
        use super::{CStr, File, c_int, ffi, io};
        use std::{
            ffi::c_void,
            mem::{offset_of, size_of},
            os::{fd::BorrowedFd, unix::fs::MetadataExt as _},
        };

        #[derive(Clone, Copy, Debug, Eq, PartialEq)]
        pub(super) struct FileIdentity {
            device: u64,
            inode: u64,
        }

        /// Reviewed prefix of `SQLite` 3.53.4's internal `unixFile`.
        ///
        /// The source-pinned amalgamation declares these four fields first and
        /// unconditionally. No later layout field is read. Runtime source-id
        /// admission and the size/offset assertions prevent applying this
        /// private prefix to an unreviewed engine build.
        #[repr(C)]
        pub(super) struct UnixFilePrefix {
            methods: *const ffi::sqlite3_io_methods,
            vfs: *mut ffi::sqlite3_vfs,
            inode_state: *mut c_void,
            descriptor: c_int,
        }

        const _: () = assert!(offset_of!(UnixFilePrefix, methods) == 0);
        const _: () =
            assert!(offset_of!(UnixFilePrefix, descriptor) == 3 * size_of::<*mut c_void>());

        pub(super) fn stock_vfs_name() -> &'static CStr {
            c"unix"
        }

        pub(super) fn retained_identity(file: &File) -> io::Result<FileIdentity> {
            let metadata = file.metadata()?;
            if !metadata.is_file() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "retained SQLite authority is not a regular file",
                ));
            }
            Ok(FileIdentity {
                device: metadata.dev(),
                inode: metadata.ino(),
            })
        }

        pub(super) unsafe fn sqlite_file_identity(
            file: *mut ffi::sqlite3_file,
        ) -> Result<FileIdentity, ()> {
            if file.is_null() {
                return Err(());
            }
            // SAFETY: exact_x_open calls this only after the reviewed Unix VFS
            // successfully initialized a sqlite3_file whose allocation is at
            // least sizeof(UnixFilePrefix), checked at registration.
            let prefix = unsafe { &*(file.cast::<UnixFilePrefix>()) };
            if prefix.descriptor < 0 {
                return Err(());
            }
            // SAFETY: the descriptor is owned by the live sqlite3_file for the
            // duration of xOpen. BorrowedFd does not close or outlive it.
            let descriptor = unsafe { BorrowedFd::borrow_raw(prefix.descriptor) };
            let stat = rustix::fs::fstat(descriptor).map_err(|_| ())?;
            // libc's st_dev ABI type varies by Unix target. The checked
            // conversion is meaningful on some supported targets and an
            // identity conversion on Linux.
            #[allow(clippy::useless_conversion)]
            let device = u64::try_from(stat.st_dev).map_err(|_| ())?;
            Ok(FileIdentity {
                device,
                inode: stat.st_ino,
            })
        }
    }

    #[cfg(windows)]
    mod platform {
        use super::{CStr, File, ffi, io, ptr};
        use std::{mem::MaybeUninit, os::windows::io::AsRawHandle as _};
        use windows_sys::Win32::{
            Foundation::{HANDLE, INVALID_HANDLE_VALUE},
            Storage::FileSystem::{FILE_ID_INFO, FileIdInfo, GetFileInformationByHandleEx},
        };

        #[derive(Clone, Copy, Debug, Eq, PartialEq)]
        pub(super) struct FileIdentity {
            volume: u64,
            file: [u8; 16],
        }

        pub(super) fn stock_vfs_name() -> &'static CStr {
            c"win32"
        }

        pub(super) fn retained_identity(file: &File) -> io::Result<FileIdentity> {
            if !file.metadata()?.is_file() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "retained SQLite authority is not a regular file",
                ));
            }
            file_identity(file.as_raw_handle() as HANDLE)
        }

        fn file_identity(handle: HANDLE) -> io::Result<FileIdentity> {
            if handle.is_null() || handle == INVALID_HANDLE_VALUE {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "SQLite native handle is invalid",
                ));
            }
            let mut information = MaybeUninit::<FILE_ID_INFO>::uninit();
            // SAFETY: handle is checked and remains borrowed for this
            // synchronous call. information is aligned writable storage and is
            // read only after the kernel reports complete initialization.
            if unsafe {
                GetFileInformationByHandleEx(
                    handle,
                    FileIdInfo,
                    information.as_mut_ptr().cast(),
                    u32::try_from(std::mem::size_of::<FILE_ID_INFO>())
                        .map_err(|_| io::Error::other("FILE_ID_INFO size"))?,
                )
            } == 0
            {
                return Err(io::Error::last_os_error());
            }
            // SAFETY: the successful kernel call initialized FILE_ID_INFO.
            let information = unsafe { information.assume_init() };
            Ok(FileIdentity {
                volume: information.VolumeSerialNumber,
                file: information.FileId.Identifier,
            })
        }

        pub(super) unsafe fn sqlite_file_identity(
            file: *mut ffi::sqlite3_file,
        ) -> Result<FileIdentity, ()> {
            if file.is_null() {
                return Err(());
            }
            // SAFETY: exact_x_open calls this after successful stock win32
            // xOpen, which installs a live method table in file.
            let methods = unsafe { (*file).pMethods };
            if methods.is_null() {
                return Err(());
            }
            // SAFETY: methods is the stock live table for file.
            let Some(control) = (unsafe { (*methods).xFileControl }) else {
                return Err(());
            };
            let mut handle: HANDLE = ptr::null_mut();
            // SAFETY: control is the stock method for this live file. The
            // documented WIN32_GET_HANDLE opcode writes one borrowed HANDLE to
            // the aligned handle variable.
            if unsafe {
                control(
                    file,
                    ffi::SQLITE_FCNTL_WIN32_GET_HANDLE,
                    (&raw mut handle).cast(),
                )
            } != ffi::SQLITE_OK
            {
                return Err(());
            }
            file_identity(handle).map_err(|_| ())
        }
    }

    #[cfg(not(any(unix, windows)))]
    compile_error!("worldstream-sqlite-open supports only Unix and Windows");

    #[cfg(test)]
    mod tests {
        use super::{
            AfterBaseOpenHook, BeforeOwnerOpenHook, Connection, ExactSqliteConnection,
            ExactSqliteOpenError, File, OpenFlags, Path, ffi, io, open_exact, open_exact_inner,
        };
        use std::{
            collections::HashSet, ffi::CString, fs, fs::OpenOptions, io::Write as _, thread,
        };
        use tempfile::tempdir;

        fn retained(path: &Path) -> io::Result<File> {
            OpenOptions::new().read(true).write(true).open(path)
        }

        fn flags() -> OpenFlags {
            OpenFlags::SQLITE_OPEN_READ_WRITE
                | OpenFlags::SQLITE_OPEN_NO_MUTEX
                | OpenFlags::SQLITE_OPEN_NOFOLLOW
        }

        fn assert_send<T: Send>() {}

        #[test]
        fn exact_connection_preserves_rusqlite_send_contract() {
            assert_send::<ExactSqliteConnection>();
        }

        #[test]
        fn exact_main_database_preserves_stock_wal_behavior() {
            let directory = tempdir().unwrap_or_else(|error| unreachable!("tempdir: {error}"));
            let root = fs::canonicalize(directory.path())
                .unwrap_or_else(|error| unreachable!("canonical tempdir: {error}"));
            let path = root.join("database.sqlite3");
            File::create(&path).unwrap_or_else(|error| unreachable!("database: {error}"));
            let authority =
                retained(&path).unwrap_or_else(|error| unreachable!("retained database: {error}"));
            let connection = open_exact(authority, &path, flags())
                .unwrap_or_else(|error| unreachable!("exact open: {error}"));
            let mode: String = connection
                .query_row("PRAGMA journal_mode=WAL", (), |row| row.get(0))
                .unwrap_or_else(|error| unreachable!("WAL mode: {error}"));
            assert_eq!(mode, "wal");
            connection
                .execute_batch(
                    "PRAGMA synchronous=FULL; CREATE TABLE exact(value TEXT NOT NULL); \
                     INSERT INTO exact(value) VALUES ('bound');",
                )
                .unwrap_or_else(|error| unreachable!("write exact database: {error}"));
            assert!(Path::new(&format!("{}-wal", path.display())).exists());
            let value: String = connection
                .query_row("SELECT value FROM exact", (), |row| row.get(0))
                .unwrap_or_else(|error| unreachable!("read exact database: {error}"));
            assert_eq!(value, "bound");
            drop(connection);

            let authority = retained(&path)
                .unwrap_or_else(|error| unreachable!("reopen retained database: {error}"));
            let reopened = open_exact(authority, &path, flags())
                .unwrap_or_else(|error| unreachable!("reopen exact: {error}"));
            let value: String = reopened
                .query_row("SELECT value FROM exact", (), |row| row.get(0))
                .unwrap_or_else(|error| unreachable!("read reopened database: {error}"));
            assert_eq!(value, "bound");
        }

        #[test]
        fn preopen_path_substitution_cannot_redirect_the_connection() {
            let directory = tempdir().unwrap_or_else(|error| unreachable!("tempdir: {error}"));
            let root = fs::canonicalize(directory.path())
                .unwrap_or_else(|error| unreachable!("canonical tempdir: {error}"));
            let path = root.join("database.sqlite3");
            let held = root.join("held.sqlite3");
            fs::write(&path, b"retained bytes")
                .unwrap_or_else(|error| unreachable!("retained bytes: {error}"));
            let authority =
                retained(&path).unwrap_or_else(|error| unreachable!("retained database: {error}"));
            fs::rename(&path, &held)
                .unwrap_or_else(|error| unreachable!("hold retained database: {error}"));
            fs::write(&path, b"replacement bytes")
                .unwrap_or_else(|error| unreachable!("replacement bytes: {error}"));

            let result = open_exact(authority, &path, flags());
            assert!(matches!(
                result,
                Err(ExactSqliteOpenError::MainDatabaseIdentityMismatch)
            ));
            assert_eq!(
                fs::read(&held).unwrap_or_else(|error| unreachable!("held bytes: {error}")),
                b"retained bytes"
            );
            assert_eq!(
                fs::read(&path).unwrap_or_else(|error| unreachable!("replacement bytes: {error}")),
                b"replacement bytes"
            );
            assert!(!Path::new(&format!("{}-wal", path.display())).exists());
            assert!(!Path::new(&format!("{}-shm", path.display())).exists());
        }

        #[cfg(unix)]
        #[test]
        fn transient_substitution_restored_before_validation_still_rejects() {
            let directory = tempdir().unwrap_or_else(|error| unreachable!("tempdir: {error}"));
            let root = fs::canonicalize(directory.path())
                .unwrap_or_else(|error| unreachable!("canonical tempdir: {error}"));
            let path = root.join("database.sqlite3");
            let held = root.join("held-retained.sqlite3");
            let replacement_held = root.join("held-replacement.sqlite3");
            fs::write(&path, b"retained bytes")
                .unwrap_or_else(|error| unreachable!("retained bytes: {error}"));
            let authority =
                retained(&path).unwrap_or_else(|error| unreachable!("retained database: {error}"));
            fs::rename(&path, &held)
                .unwrap_or_else(|error| unreachable!("hold retained database: {error}"));
            fs::write(&path, b"replacement bytes")
                .unwrap_or_else(|error| unreachable!("replacement bytes: {error}"));
            let hook_path = path.clone();
            let hook_held = held.clone();
            let hook_replacement = replacement_held.clone();
            let hook: AfterBaseOpenHook = Box::new(move || {
                fs::rename(&hook_path, &hook_replacement)?;
                fs::rename(&hook_held, &hook_path)
            });

            let result = open_exact_inner(authority, &path, flags(), None, Some(hook));
            assert!(matches!(
                result,
                Err(ExactSqliteOpenError::MainDatabaseIdentityMismatch)
            ));
            assert_eq!(
                fs::read(&path)
                    .unwrap_or_else(|error| unreachable!("restored retained bytes: {error}")),
                b"retained bytes"
            );
            assert_eq!(
                fs::read(&replacement_held)
                    .unwrap_or_else(|error| unreachable!("held replacement bytes: {error}")),
                b"replacement bytes"
            );
        }

        #[test]
        fn create_flag_is_rejected_before_path_mutation() {
            let directory = tempdir().unwrap_or_else(|error| unreachable!("tempdir: {error}"));
            let root = fs::canonicalize(directory.path())
                .unwrap_or_else(|error| unreachable!("canonical tempdir: {error}"));
            let path = root.join("database.sqlite3");
            let mut authority = tempfile::tempfile()
                .unwrap_or_else(|error| unreachable!("anonymous authority: {error}"));
            authority
                .write_all(b"authority")
                .unwrap_or_else(|error| unreachable!("authority bytes: {error}"));
            let result = open_exact(authority, &path, flags() | OpenFlags::SQLITE_OPEN_CREATE);
            assert!(matches!(
                result,
                Err(ExactSqliteOpenError::InvalidOpenFlags)
            ));
            assert!(!path.exists());
        }

        #[test]
        fn unsafe_or_ambiguous_open_flags_are_rejected_before_open() {
            let directory = tempdir().unwrap_or_else(|error| unreachable!("tempdir: {error}"));
            let root = fs::canonicalize(directory.path())
                .unwrap_or_else(|error| unreachable!("canonical tempdir: {error}"));
            let path = root.join("database.sqlite3");
            fs::write(&path, b"unchanged")
                .unwrap_or_else(|error| unreachable!("database bytes: {error}"));
            let invalid = [
                OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
                OpenFlags::SQLITE_OPEN_READ_ONLY
                    | OpenFlags::SQLITE_OPEN_READ_WRITE
                    | OpenFlags::SQLITE_OPEN_NOFOLLOW,
                OpenFlags::SQLITE_OPEN_NOFOLLOW,
                flags() | OpenFlags::SQLITE_OPEN_CREATE,
                flags() | OpenFlags::SQLITE_OPEN_URI,
                flags() | OpenFlags::SQLITE_OPEN_MEMORY,
                flags() | OpenFlags::SQLITE_OPEN_FULL_MUTEX,
                flags() | OpenFlags::SQLITE_OPEN_SHARED_CACHE,
                flags() | OpenFlags::from_bits_retain(0x4000_0000),
            ];
            for candidate in invalid {
                let authority = retained(&path)
                    .unwrap_or_else(|error| unreachable!("retained database: {error}"));
                assert!(matches!(
                    open_exact(authority, &path, candidate),
                    Err(ExactSqliteOpenError::InvalidOpenFlags)
                ));
            }
            assert_eq!(
                fs::read(&path).unwrap_or_else(|error| unreachable!("database bytes: {error}")),
                b"unchanged"
            );
        }

        #[test]
        fn retained_read_only_database_is_accepted_without_write_authority() {
            let directory = tempdir().unwrap_or_else(|error| unreachable!("tempdir: {error}"));
            let root = fs::canonicalize(directory.path())
                .unwrap_or_else(|error| unreachable!("canonical tempdir: {error}"));
            let path = root.join("database.sqlite3");
            File::create(&path).unwrap_or_else(|error| unreachable!("database: {error}"));
            let writer = open_exact(
                retained(&path).unwrap_or_else(|error| unreachable!("retained writer: {error}")),
                &path,
                flags(),
            )
            .unwrap_or_else(|error| unreachable!("writer open: {error}"));
            writer
                .execute_batch(
                    "CREATE TABLE exact(value INTEGER NOT NULL); INSERT INTO exact VALUES (7);",
                )
                .unwrap_or_else(|error| unreachable!("seed database: {error}"));
            drop(writer);

            let authority = OpenOptions::new()
                .read(true)
                .open(&path)
                .unwrap_or_else(|error| unreachable!("read-only authority: {error}"));
            let reader = open_exact(
                authority,
                &path,
                OpenFlags::SQLITE_OPEN_READ_ONLY
                    | OpenFlags::SQLITE_OPEN_NO_MUTEX
                    | OpenFlags::SQLITE_OPEN_NOFOLLOW,
            )
            .unwrap_or_else(|error| unreachable!("reader open: {error}"));
            let value: i64 = reader
                .query_row("SELECT value FROM exact", (), |row| row.get(0))
                .unwrap_or_else(|error| unreachable!("reader query: {error}"));
            assert_eq!(value, 7);
            assert!(reader.execute("DELETE FROM exact", ()).is_err());
        }

        #[test]
        fn concurrent_private_vfs_names_unregister_after_connections_close() {
            const CONNECTIONS: usize = 8;
            let directory = tempdir().unwrap_or_else(|error| unreachable!("tempdir: {error}"));
            let root = fs::canonicalize(directory.path())
                .unwrap_or_else(|error| unreachable!("canonical tempdir: {error}"));
            let mut workers = Vec::with_capacity(CONNECTIONS);
            for index in 0..CONNECTIONS {
                let path = root.join(format!("database-{index}.sqlite3"));
                File::create(&path).unwrap_or_else(|error| unreachable!("database: {error}"));
                workers.push(thread::spawn(move || {
                    let authority = retained(&path).map_err(|error| error.to_string())?;
                    let connection =
                        open_exact(authority, &path, flags()).map_err(|error| error.to_string())?;
                    connection
                        .execute_batch("CREATE TABLE exact(value INTEGER NOT NULL);")
                        .map_err(|error| error.to_string())?;
                    let name = connection
                        .registered_vfs
                        .name_cstr()
                        .ok_or_else(|| "registered VFS name unavailable".to_owned())?
                        .to_owned();
                    Ok::<_, String>((connection, name))
                }));
            }

            let mut connections = Vec::with_capacity(CONNECTIONS);
            let mut names = Vec::<CString>::with_capacity(CONNECTIONS);
            for worker in workers {
                let result = worker
                    .join()
                    .unwrap_or_else(|_| unreachable!("exact-open worker panicked"))
                    .unwrap_or_else(|error| unreachable!("exact-open worker: {error}"));
                connections.push(result.0);
                names.push(result.1);
            }
            assert_eq!(
                names
                    .iter()
                    .map(|name| name.as_bytes().to_vec())
                    .collect::<HashSet<_>>()
                    .len(),
                CONNECTIONS
            );
            for name in &names {
                // SAFETY: name is a live NUL-terminated private VFS name. The
                // returned pointer is observed only for nullness.
                assert!(!unsafe { ffi::sqlite3_vfs_find(name.as_ptr()) }.is_null());
            }
            drop(connections);
            for name in &names {
                // SAFETY: name remains a live NUL-terminated string and the
                // lookup result is observed only for nullness.
                assert!(unsafe { ffi::sqlite3_vfs_find(name.as_ptr()) }.is_null());
            }
        }

        #[test]
        fn foreign_connection_cannot_capture_exact_admission_before_owner_open() {
            use std::sync::{Arc, Mutex};

            let directory = tempdir().unwrap_or_else(|error| unreachable!("tempdir: {error}"));
            let root = fs::canonicalize(directory.path())
                .unwrap_or_else(|error| unreachable!("canonical tempdir: {error}"));
            let path = root.join("database.sqlite3");
            File::create(&path).unwrap_or_else(|error| unreachable!("database: {error}"));
            let authority =
                retained(&path).unwrap_or_else(|error| unreachable!("retained database: {error}"));
            let foreign = Arc::new(Mutex::new(None));
            let hook_foreign = Arc::clone(&foreign);
            let hook_path = path.clone();
            let hook: BeforeOwnerOpenHook = Box::new(move |name| {
                if let Ok(connection) =
                    Connection::open_with_flags_and_vfs(&hook_path, flags(), name)
                {
                    *hook_foreign
                        .lock()
                        .map_err(|_| io::Error::other("foreign connection slot"))? =
                        Some(connection);
                }
                Ok(())
            });

            let owner = open_exact_inner(authority, &path, flags(), Some(hook), None);
            let foreign = foreign
                .lock()
                .unwrap_or_else(|_| unreachable!("foreign connection slot"))
                .take();
            let foreign_opened = foreign.is_some();
            // The pre-fix failure leaves this connection holding a pointer to
            // the freed per-owner VFS. Do not invoke or close that pointer in
            // the red-capable test process.
            if let Some(connection) = foreign {
                std::mem::forget(connection);
            }
            assert!(
                !foreign_opened,
                "a foreign connection captured exact admission"
            );
            assert!(owner.is_ok(), "the exact owner failed to open");
        }
    }
}

pub use implementation::{ExactSqliteConnection, ExactSqliteOpenError, open_exact};
