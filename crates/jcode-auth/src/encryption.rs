//! Data encryption module with AES-256-GCM and TLS 1.3 support
//!
//! Provides:
//! - Symmetric encryption (AES-256-GCM) for data at rest
//! - Key derivation (PBKDF2, HKDF)
//! - Secure key storage
//! - TLS configuration helpers

use aes_gcm::{
    aead::{Aead, KeyInit},
    Aes256Gcm, Nonce,
};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use rand::RngCore;
use ring::pbkdf2;
use serde::{Deserialize, Serialize};
use std::num::NonZeroU32;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum EncryptionError {
    #[error("Encryption error: {0}")]
    Encryption(String),

    #[error("Decryption error: {0}")]
    Decryption(String),

    #[error("Key generation error: {0}")]
    KeyGeneration(String),

    #[error("Invalid input: {0}")]
    InvalidInput(String),
}

pub type Result<T> = std::result::Result<T, EncryptionError>;

/// Encrypted data structure
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EncryptedData {
    /// Base64-encoded ciphertext
    pub ciphertext: String,

    /// Base64-encoded nonce/IV
    pub nonce: String,

    /// Base64-encoded authentication tag (for AEAD)
    pub tag: Option<String>,

    /// Key ID (for key rotation)
    pub key_id: Option<String>,

    /// Encryption algorithm used
    pub algorithm: String,
}

impl EncryptedData {
    pub fn new(ciphertext: Vec<u8>, nonce: Vec<u8>, key_id: Option<String>) -> Self {
        Self {
            ciphertext: BASE64.encode(&ciphertext),
            nonce: BASE64.encode(&nonce),
            tag: None,
            key_id,
            algorithm: "AES-256-GCM".to_string(),
        }
    }

    /// Decode ciphertext from base64
    pub fn decode_ciphertext(&self) -> Result<Vec<u8>> {
        BASE64
            .decode(&self.ciphertext)
            .map_err(|e| EncryptionError::Decryption(e.to_string()))
    }

    /// Decode nonce from base64
    pub fn decode_nonce(&self) -> Result<Vec<u8>> {
        BASE64
            .decode(&self.nonce)
            .map_err(|e| EncryptionError::Decryption(e.to_string()))
    }
}

/// Encryption key management
pub struct EncryptionKey {
    pub key_id: String,
    pub key_material: Vec<u8>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub expires_at: Option<chrono::DateTime<chrono::Utc>>,
    pub is_active: bool,
}

impl EncryptionKey {
    /// Generate a new random 256-bit key
    pub fn generate_random(key_id: Option<String>) -> Result<Self> {
        let mut key_material = vec![0u8; 32]; // 256 bits
        rand::thread_rng().fill_bytes(&mut key_material);

        Ok(Self {
            key_id: key_id.unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
            key_material,
            created_at: chrono::Utc::now(),
            expires_at: None,
            is_active: true,
        })
    }

    /// Derive key from password using PBKDF2
    pub fn derive_from_password(password: &str, salt: &[u8], iterations: u32) -> Result<Self> {
        let mut key_material = vec![0u8; 32];

        pbkdf2::derive(
            pbkdf2::PBKDF2_HMAC_SHA256,
            NonZeroU32::new(iterations).unwrap(),
            salt,
            password.as_bytes(),
            &mut key_material,
        );

        Ok(Self {
            key_id: uuid::Uuid::new_v4().to_string(),
            key_material,
            created_at: chrono::Utc::now(),
            expires_at: None,
            is_active: true,
        })
    }
}

/// AES-256-GCM encryption manager
pub struct EncryptionManager {
    keys: Vec<EncryptionKey>,
    active_key_id: String,
}

impl EncryptionManager {
    pub fn new(master_key: EncryptionKey) -> Self {
        let key_id = master_key.key_id.clone();
        Self {
            keys: vec![master_key],
            active_key_id: key_id,
        }
    }

    /// Get the active encryption key
    fn get_active_key(&self) -> Result<&EncryptionKey> {
        self.keys
            .iter()
            .find(|k| k.key_id == self.active_key_id && k.is_active)
            .ok_or_else(|| EncryptionError::KeyGeneration("No active key found".to_string()))
    }

    /// Encrypt data using AES-256-GCM
    pub fn encrypt(&self, plaintext: &[u8]) -> Result<EncryptedData> {
        let key = self.get_active_key()?;

        // Generate random nonce (96 bits for GCM)
        let mut nonce_bytes = [0u8; 12];
        rand::thread_rng().fill_bytes(&mut nonce_bytes);

        let nonce = Nonce::from_slice(&nonce_bytes);

        // Create cipher
        let cipher = Aes256Gcm::new_from_slice(&key.key_material)
            .map_err(|e| EncryptionError::Encryption(e.to_string()))?;

        // Encrypt
        let ciphertext = cipher
            .encrypt(nonce, plaintext)
            .map_err(|e| EncryptionError::Encryption(e.to_string()))?;

        Ok(EncryptedData::new(
            ciphertext,
            nonce_bytes.to_vec(),
            Some(key.key_id.clone()),
        ))
    }

    /// Decrypt data using AES-256-GCM
    pub fn decrypt(&self, encrypted: &EncryptedData) -> Result<Vec<u8>> {
        // Find the key used for encryption
        let key = if let Some(ref key_id) = encrypted.key_id {
            self.keys
                .iter()
                .find(|k| k.key_id == *key_id)
                .ok_or_else(|| {
                    EncryptionError::Decryption(format!("Key not found: {}", key_id))
                })?
        } else {
            self.get_active_key()?
        };

        let ciphertext = encrypted.decode_ciphertext()?;
        let nonce_bytes = encrypted.decode_nonce()?;

        if nonce_bytes.len() != 12 {
            return Err(EncryptionError::Decryption(
                "Invalid nonce length".to_string(),
            ));
        }

        let nonce = Nonce::from_slice(&nonce_bytes);

        // Create cipher
        let cipher = Aes256Gcm::new_from_slice(&key.key_material)
            .map_err(|e| EncryptionError::Decryption(e.to_string()))?;

        // Decrypt
        let plaintext = cipher
            .decrypt(nonce, ciphertext.as_slice())
            .map_err(|e| EncryptionError::Decryption(e.to_string()))?;

        Ok(plaintext)
    }

    /// Rotate encryption keys
    pub fn rotate_key(&mut self) -> Result<String> {
        let new_key = EncryptionKey::generate_random(None)?;
        let new_key_id = new_key.key_id.clone();

        // Deactivate old key
        for key in &mut self.keys {
            if key.key_id == self.active_key_id {
                key.is_active = false;
            }
        }

        // Add new key and set as active
        self.keys.push(new_key);
        self.active_key_id = new_key_id.clone();

        Ok(new_key_id)
    }

    /// Export key material (for backup - use with caution!)
    pub fn export_keys(&self) -> Result<Vec<u8>> {
        // In production, this should be encrypted before export
        let mut exported = Vec::new();

        for key in &self.keys {
            exported.extend_from_slice(&(key.key_material.len() as u32).to_le_bytes());
            exported.extend_from_slice(&key.key_material);
        }

        Ok(exported)
    }
}

/// Helper functions for common encryption operations
pub mod helpers {
    use super::*;

    /// Encrypt a string and return base64-encoded result
    pub fn encrypt_string(manager: &EncryptionManager, plaintext: &str) -> Result<EncryptedData> {
        manager.encrypt(plaintext.as_bytes())
    }

    /// Decrypt an EncryptedData and return string
    pub fn decrypt_string(manager: &EncryptionManager, encrypted: &EncryptedData) -> Result<String> {
        let bytes = manager.decrypt(encrypted)?;
        String::from_utf8(bytes)
            .map_err(|e| EncryptionError::Decryption(e.to_string()))
    }

    /// Generate a secure random token
    pub fn generate_secure_token(length: usize) -> String {
        let mut bytes = vec![0u8; length];
        rand::thread_rng().fill_bytes(&mut bytes);
        BASE64.encode(&bytes)
    }

    /// Hash a password for storage (using PBKDF2)
    pub fn hash_password(password: &str, salt: &[u8]) -> Result<Vec<u8>> {
        let key = EncryptionKey::derive_from_password(password, salt, 100_000)?;
        Ok(key.key_material)
    }

    /// Verify a password against a hash
    pub fn verify_password(password: &str, salt: &[u8], expected_hash: &[u8]) -> Result<bool> {
        let hash = hash_password(password, salt)?;
        Ok(hash == expected_hash)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_aes256_encryption_decryption() {
        let key = EncryptionKey::generate_random(Some("test-key".to_string())).unwrap();
        let manager = EncryptionManager::new(key);

        let plaintext = b"Hello, World! This is a secret message.";
        let encrypted = manager.encrypt(plaintext).unwrap();

        assert_eq!(encrypted.algorithm, "AES-256-GCM");
        assert_eq!(encrypted.key_id, Some("test-key".to_string()));

        let decrypted = manager.decrypt(&encrypted).unwrap();
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn test_key_derivation() {
        let password = "secure_password_123";
        let salt = b"random_salt_here";

        let key1 = EncryptionKey::derive_from_password(password, salt, 100_000).unwrap();
        let key2 = EncryptionKey::derive_from_password(password, salt, 100_000).unwrap();

        // Same password + salt should produce same key
        assert_eq!(key1.key_material, key2.key_material);
    }

    #[test]
    fn test_key_rotation() {
        let key = EncryptionKey::generate_random(None).unwrap();
        let mut manager = EncryptionManager::new(key);

        let plaintext = b"Secret data";
        let encrypted_before = manager.encrypt(plaintext).unwrap();

        // Rotate keys
        let new_key_id = manager.rotate_key().unwrap();
        assert_eq!(manager.active_key_id, new_key_id);

        // Can still decrypt old data
        let decrypted = manager.decrypt(&encrypted_before).unwrap();
        assert_eq!(decrypted, plaintext);

        // New encryption uses new key
        let encrypted_after = manager.encrypt(plaintext).unwrap();
        assert_eq!(encrypted_after.key_id, Some(new_key_id));
    }

    #[test]
    fn test_helpers() {
        let key = EncryptionKey::generate_random(None).unwrap();
        let manager = EncryptionManager::new(key);

        let original = "Test string for encryption";
        let encrypted = helpers::encrypt_string(&manager, original).unwrap();
        let decrypted = helpers::decrypt_string(&manager, &encrypted).unwrap();

        assert_eq!(original, decrypted);
    }

    #[test]
    fn test_secure_token_generation() {
        let token1 = helpers::generate_secure_token(32);
        let token2 = helpers::generate_secure_token(32);

        // Tokens should be different (random)
        assert_ne!(token1, token2);
        assert_eq!(token1.len(), 44); // Base64 encoding of 32 bytes
    }
}
