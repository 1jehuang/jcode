use thiserror::Error;

#[derive(Error, Debug)]
pub enum LockManagerError {
    #[error("lock with id {0} not found")]
    LockNotFound(u32),
    
    #[error("lock with name {0} not found")]
    LockByNameNotFound(String),
    
    #[error("no locks found with type {0}")]
    LocksByTypeNotFound(String),
    
    #[error("failed to acquire read lock: {0}")]
    ReadLockAcquisitionFailed(String),
    
    #[error("failed to acquire write lock: {0}")]
    WriteLockAcquisitionFailed(String),
    
    #[error("lock contention detected: {0}")]
    LockContention(String),
    
    #[error("potential deadlock detected: {0}")]
    PotentialDeadlock(String),
    
    #[error("internal error: {0}")]
    Internal(String),
}

impl From<std::io::Error> for LockManagerError {
    fn from(e: std::io::Error) -> Self {
        LockManagerError::Internal(e.to_string())
    }
}

impl From<tokio::sync::AcquireError> for LockManagerError {
    fn from(e: tokio::sync::AcquireError) -> Self {
        LockManagerError::Internal(e.to_string())
    }
}
