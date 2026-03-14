use std::sync::{LockResult, MutexGuard, RwLockReadGuard, RwLockWriteGuard};

/// Extension trait for `LockResult` that logs an error when recovering from a
/// poisoned mutex/rwlock instead of silently swallowing the poison event.
pub trait UnpoisonExt<T> {
    fn unwrap_or_log(self) -> T;
}

impl<'a, T> UnpoisonExt<MutexGuard<'a, T>> for LockResult<MutexGuard<'a, T>> {
    fn unwrap_or_log(self) -> MutexGuard<'a, T> {
        self.unwrap_or_else(|e| {
            log::error!("Recovering from poisoned Mutex lock");
            e.into_inner()
        })
    }
}

impl<'a, T> UnpoisonExt<RwLockReadGuard<'a, T>> for LockResult<RwLockReadGuard<'a, T>> {
    fn unwrap_or_log(self) -> RwLockReadGuard<'a, T> {
        self.unwrap_or_else(|e| {
            log::error!("Recovering from poisoned RwLock (read) lock");
            e.into_inner()
        })
    }
}

impl<'a, T> UnpoisonExt<RwLockWriteGuard<'a, T>> for LockResult<RwLockWriteGuard<'a, T>> {
    fn unwrap_or_log(self) -> RwLockWriteGuard<'a, T> {
        self.unwrap_or_else(|e| {
            log::error!("Recovering from poisoned RwLock (write) lock");
            e.into_inner()
        })
    }
}
