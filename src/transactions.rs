use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::time::UNIX_EPOCH;

/// Represents the state of a file before any changes
#[derive(Clone, Debug)]
pub struct FileSnapshot {
    pub path: String,
    pub content: Option<String>, // None if file didn't exist
    pub modified: Option<u64>,   // Unix timestamp, None if file didn't exist
    pub exists: bool,
}

impl FileSnapshot {
    /// Create a snapshot of a file's current state
    pub fn snapshot(path: &str) -> std::io::Result<Self> {
        let path_obj = Path::new(path);

        let exists = path_obj.exists();
        let content = if exists {
            Some(fs::read_to_string(path)?)
        } else {
            None
        };

        let modified = if exists {
            path_obj.metadata()?
                .modified()?
                .duration_since(UNIX_EPOCH)
                .ok()
                .map(|d| d.as_secs())
        } else {
            None
        };

        Ok(FileSnapshot {
            path: path.to_string(),
            content,
            modified,
            exists,
        })
    }

    /// Restore the file to its snapshot state
    pub fn restore(&self) -> std::io::Result<()> {
        if self.exists {
            // File existed, restore content and timestamp
            if let Some(content) = &self.content {
                fs::write(&self.path, content)?;
            }
            // Note: We don't restore timestamps as they're not critical for rollback
        } else {
            // File didn't exist, remove it if it now exists
            if Path::new(&self.path).exists() {
                fs::remove_file(&self.path)?;
            }
        }
        Ok(())
    }
}

/// Manages a single transaction's file changes
#[derive(Debug)]
pub struct Transaction {
    pub snapshots: HashMap<String, FileSnapshot>,
    pub modified_files: Vec<String>,
}

impl Transaction {
    pub fn new() -> Self {
        Transaction {
            snapshots: HashMap::new(),
            modified_files: Vec::new(),
        }
    }

    /// Take a snapshot of a file before it might be modified
    pub fn snapshot_file(&mut self, path: &str) -> std::io::Result<()> {
        if !self.snapshots.contains_key(path) {
            let snapshot = FileSnapshot::snapshot(path)?;
            self.snapshots.insert(path.to_string(), snapshot);
        }
        Ok(())
    }

    /// Mark a file as modified during this transaction
    pub fn mark_modified(&mut self, path: &str) {
        if !self.modified_files.contains(&path.to_string()) {
            self.modified_files.push(path.to_string());
        }
    }

    /// Rollback all changes made during this transaction
    pub fn rollback(&self) -> std::io::Result<()> {
        for snapshot in self.snapshots.values() {
            snapshot.restore()?;
        }
        Ok(())
    }
}

/// Global transaction manager
pub struct TransactionManager {
    current_transaction: Option<Transaction>,
    sandbox_cwd: Option<String>,
}

impl TransactionManager {
    pub fn new() -> Self {
        TransactionManager {
            current_transaction: None,
            sandbox_cwd: None,
        }
    }

    pub fn set_sandbox(&mut self, cwd: Option<String>) {
        self.sandbox_cwd = cwd;
    }

    /// Start a new transaction
    pub fn begin_transaction(&mut self) {
        self.current_transaction = Some(Transaction::new());
    }

    /// End the current transaction (commit - just clear it)
    pub fn commit_transaction(&mut self) {
        self.current_transaction = None;
    }

    /// Rollback the current transaction and restore all files
    pub fn rollback_transaction(&mut self) -> std::io::Result<()> {
        if let Some(ref transaction) = self.current_transaction {
            transaction.rollback()?;
        }
        self.current_transaction = None;
        Ok(())
    }

    /// Check if a path is allowed (sandbox mode)
    fn is_path_allowed(&self, path: &str) -> bool {
        if let Some(ref cwd) = self.sandbox_cwd {
            path.starts_with(cwd) || Path::new(path).is_absolute() == false
        } else {
            true
        }
    }

    /// Prepare a file for modification (take snapshot if not already done)
    pub fn prepare_file(&mut self, path: &str) -> std::io::Result<()> {
        if !self.is_path_allowed(path) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                format!("Cannot modify files outside of sandbox: {}", path)
            ));
        }

        if let Some(ref mut transaction) = self.current_transaction {
            transaction.snapshot_file(path)?;
        }
        Ok(())
    }

    /// Mark a file as modified
    pub fn mark_file_modified(&mut self, path: &str) {
        if let Some(ref mut transaction) = self.current_transaction {
            transaction.mark_modified(path);
        }
    }

    /// Execute a file operation within the transaction context
    pub fn execute_file_operation<F, R>(&mut self, path: &str, operation: F) -> std::io::Result<R>
    where
        F: FnOnce() -> std::io::Result<R>,
    {
        self.prepare_file(path)?;
        let result = operation()?;
        self.mark_file_modified(path);
        Ok(result)
    }

    /// Check if we're currently in a transaction
    pub fn in_transaction(&self) -> bool {
        self.current_transaction.is_some()
    }

    /// Get transaction status for debugging
    pub fn get_transaction_status(&self) -> String {
        if let Some(ref transaction) = self.current_transaction {
            format!(
                "Transaction active: {} files tracked, {} modified",
                transaction.snapshots.len(),
                transaction.modified_files.len()
            )
        } else {
            "No active transaction".to_string()
        }
    }
}

// Global instance
lazy_static::lazy_static! {
    pub static ref TRANSACTION_MANAGER: std::sync::Mutex<TransactionManager> =
        std::sync::Mutex::new(TransactionManager::new());
}

/// Initialize the transaction manager for the current session
pub fn init_transaction_manager(sandbox_cwd: Option<String>) {
    if let Ok(mut manager) = TRANSACTION_MANAGER.lock() {
        manager.set_sandbox(sandbox_cwd);
    }
}

/// Begin a new transaction
pub fn begin_transaction() {
    if let Ok(mut manager) = TRANSACTION_MANAGER.lock() {
        manager.begin_transaction();
    }
}

/// Commit the current transaction
pub fn commit_transaction() {
    if let Ok(mut manager) = TRANSACTION_MANAGER.lock() {
        manager.commit_transaction();
    }
}

/// Rollback the current transaction
pub fn rollback_transaction() -> std::io::Result<()> {
    if let Ok(mut manager) = TRANSACTION_MANAGER.lock() {
        manager.rollback_transaction()
    } else {
        Err(std::io::Error::new(std::io::ErrorKind::Other, "Could not access transaction manager"))
    }
}

/// Execute a file operation within transaction context
pub fn execute_file_operation<F, R>(path: &str, operation: F) -> std::io::Result<R>
where
    F: FnOnce() -> std::io::Result<R>,
{
    if let Ok(mut manager) = TRANSACTION_MANAGER.lock() {
        manager.execute_file_operation(path, operation)
    } else {
        Err(std::io::Error::new(std::io::ErrorKind::Other, "Could not access transaction manager"))
    }
}

/// Get transaction status
pub fn get_transaction_status() -> String {
    if let Ok(manager) = TRANSACTION_MANAGER.lock() {
        manager.get_transaction_status()
    } else {
        "Transaction manager unavailable".to_string()
    }
}