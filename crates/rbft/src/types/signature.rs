// SPDX-License-Identifier: Apache-2.0
//! Signature types and implementations for QBFT consensus.

use alloy_primitives::B256;
use alloy_rlp::{RlpDecodable, RlpEncodable};
use alloy_signer::SignerSync;
use alloy_signer_local::PrivateKeySigner;

use super::Address;

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, RlpEncodable, RlpDecodable)]
pub struct Signature {
    pub author: Address,
    pub signature: [u8; 65],
}

impl std::fmt::Debug for Signature {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.author.fmt(f)
    }
}

impl Signature {
    /// Get the author address
    pub fn author(&self) -> Address {
        self.author
    }

    /// Get the ECDSA signature bytes
    pub fn signature_bytes(&self) -> &[u8; 65] {
        &self.signature
    }

    /// Create and sign a message with the given private key, returning a Signature
    /// with both the recovered author address and the ECDSA signature
    ///
    /// Security note: This now uses proper ECDSA signing with k256::ecdsa::SigningKey
    /// Sign a message using ECDSA with the given private key.
    /// This uses alloy's signing implementation for full compatibility.
    pub fn sign_message(bytes: &[u8], private_key: &B256) -> Self {
        // Create a signer from the private key
        let signer =
            PrivateKeySigner::from_bytes(private_key).expect("Private key should be valid");

        // Get the deterministic address from the signer
        let author_address = signer.address();

        // Sign the message using alloy's signing
        let signature = signer
            .sign_message_sync(bytes)
            .expect("Message signing should succeed");

        // Convert the signature to our 65-byte format
        let signature_bytes = signature.as_bytes();

        Self {
            author: author_address,
            signature: signature_bytes,
        }
    }

    /// Verify that the signature is valid for the given message bytes
    /// This uses alloy's verification for full compatibility.
    pub fn verify_message(&self, bytes: &[u8]) -> Result<bool, Box<dyn std::error::Error>> {
        // Parse signature from 65-byte array
        let signature = alloy_primitives::Signature::try_from(&self.signature[..])?;

        // Use alloy's message recovery - this will handle EIP-191 hashing internally
        let recovered_address = signature.recover_address_from_msg(bytes)?;

        Ok(recovered_address == self.author)
    }
}

impl Default for Signature {
    fn default() -> Self {
        Self {
            author: Address::ZERO,
            signature: [0u8; 65],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::B256;

    #[test]
    fn test_verify_message_basic() {
        // Test basic signature verification
        let private_key = B256::from([
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x01,
        ]);

        let message = b"test message for verification";

        // Create signature
        let signature = Signature::sign_message(message, &private_key);

        // Verify the signature should work with the same message
        let verification_result = signature.verify_message(message);
        assert!(verification_result.is_ok(), "Verification should not error");

        let is_valid = verification_result.unwrap();
        assert!(
            is_valid,
            "Signature should be valid for the original message"
        );

        println!("✓ Basic signature verification test passed");
    }

    #[test]
    fn test_verify_message_wrong_message() {
        // Test that verification fails with wrong message
        let private_key = B256::from([
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x02,
        ]);

        let original_message = b"original message";
        let different_message = b"different message";

        // Create signature for original message
        let signature = Signature::sign_message(original_message, &private_key);

        // Verify with original message should work
        let original_result = signature.verify_message(original_message);
        assert!(
            original_result.is_ok() && original_result.unwrap(),
            "Signature should be valid for original message"
        );

        // Verify with different message should fail
        let different_result = signature.verify_message(different_message);
        assert!(different_result.is_ok(), "Verification should not error");
        assert!(
            !different_result.unwrap(),
            "Signature should be invalid for different message"
        );

        println!("✓ Wrong message verification test passed");
    }

    #[test]
    fn test_verify_message_multiple_keys() {
        // Test that signatures from different keys are distinct
        let private_key1 = B256::from([
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x03,
        ]);

        let private_key2 = B256::from([
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x04,
        ]);

        let message = b"same message signed by different keys";

        // Create signatures with different keys
        let signature1 = Signature::sign_message(message, &private_key1);
        let signature2 = Signature::sign_message(message, &private_key2);

        // Both signatures should verify with their respective keys/authors
        assert!(
            signature1.verify_message(message).unwrap(),
            "Signature1 should verify for its author"
        );
        assert!(
            signature2.verify_message(message).unwrap(),
            "Signature2 should verify for its author"
        );

        // Authors should be different
        assert_ne!(
            signature1.author(),
            signature2.author(),
            "Different private keys should produce different authors"
        );

        // Signature bytes should be different
        assert_ne!(
            signature1.signature_bytes(),
            signature2.signature_bytes(),
            "Different private keys should produce different signatures"
        );

        println!("✓ Multiple keys verification test passed");
    }

    #[test]
    fn test_verify_message_deterministic() {
        // Test that the same message with same key produces the same result
        let private_key = B256::from([
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x05,
        ]);

        let message = b"deterministic test message";

        // Create signature twice
        let signature1 = Signature::sign_message(message, &private_key);
        let signature2 = Signature::sign_message(message, &private_key);

        // Both should verify
        assert!(signature1.verify_message(message).unwrap());
        assert!(signature2.verify_message(message).unwrap());

        // They should be identical (deterministic)
        assert_eq!(signature1.author(), signature2.author());
        assert_eq!(signature1.signature_bytes(), signature2.signature_bytes());

        println!("✓ Deterministic signature test passed");
    }

    #[test]
    fn test_verify_message_empty_message() {
        // Test verification with empty message
        let private_key = B256::from([
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x06,
        ]);

        let empty_message = b"";

        // Create signature for empty message
        let signature = Signature::sign_message(empty_message, &private_key);

        // Should verify correctly
        let result = signature.verify_message(empty_message);
        assert!(
            result.is_ok(),
            "Empty message verification should not error"
        );
        assert!(result.unwrap(), "Empty message signature should be valid");

        // Should not verify with non-empty message
        let non_empty = b"not empty";
        let result2 = signature.verify_message(non_empty);
        assert!(
            result2.is_ok() && !result2.unwrap(),
            "Empty message signature should not verify for non-empty message"
        );

        println!("✓ Empty message verification test passed");
    }

    #[test]
    fn test_verify_message_large_message() {
        // Test verification with larger message
        let private_key = B256::from([
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x07,
        ]);

        // Create a larger message (1KB)
        let large_message = vec![0xAB; 1024];

        // Create signature
        let signature = Signature::sign_message(&large_message, &private_key);

        // Should verify correctly
        let result = signature.verify_message(&large_message);
        assert!(
            result.is_ok(),
            "Large message verification should not error"
        );
        assert!(result.unwrap(), "Large message signature should be valid");

        // Create a slightly different large message
        let mut different_large_message = large_message.clone();
        different_large_message[500] = 0xCD; // Change one byte

        // Should not verify with modified message
        let result2 = signature.verify_message(&different_large_message);
        assert!(
            result2.is_ok() && !result2.unwrap(),
            "Signature should not verify for modified large message"
        );

        println!("✓ Large message verification test passed");
    }

    #[test]
    fn test_verify_message_invalid_signature() {
        // Test verification with manually corrupted signature
        let private_key = B256::from([
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x08,
        ]);

        let message = b"test message for corruption";

        // Create valid signature
        let mut signature = Signature::sign_message(message, &private_key);

        // Verify it works initially
        assert!(signature.verify_message(message).unwrap());

        // Corrupt the signature bytes
        signature.signature[0] = signature.signature[0].wrapping_add(1);

        // Should now fail verification (might error or return false)
        let result = signature.verify_message(message);
        if let Ok(valid) = result {
            assert!(!valid, "Corrupted signature should not verify");
        } else {
            // It's also acceptable for corrupted signature to cause an error
            println!("Corrupted signature caused verification error (acceptable)");
        }

        println!("✓ Invalid signature test passed");
    }

    #[test]
    fn test_verify_message_comprehensive() {
        println!("\n=== Running comprehensive verify_message tests ===");

        // Test simple cases
        let simple_cases = [
            ("Hello, World!", "Simple ASCII message"),
            ("🚀 Unicode test! 日本語", "Unicode message"),
            ("", "Empty message"),
            ("Line1\nLine2\nLine3", "Multi-line message"),
            ("Special chars: !@#$%^&*()", "Special characters"),
        ];

        for (i, (message, description)) in simple_cases.iter().enumerate() {
            let private_key = B256::from({
                let mut key = [0u8; 32];
                key[31] = (i + 10) as u8; // Unique key for each test
                key
            });

            let message_bytes = message.as_bytes();

            // Create and verify signature
            let signature = Signature::sign_message(message_bytes, &private_key);
            let result = signature.verify_message(message_bytes);

            assert!(result.is_ok(), "Test '{}' should not error", description);
            assert!(result.unwrap(), "Test '{}' should verify", description);

            println!("  ✓ {}: PASS", description);
        }

        // Test large message separately
        let large_message = "x".repeat(1000);
        let large_key = B256::from([0x99; 32]);
        let large_signature = Signature::sign_message(large_message.as_bytes(), &large_key);
        let large_result = large_signature.verify_message(large_message.as_bytes());
        assert!(
            large_result.is_ok() && large_result.unwrap(),
            "Large message should verify"
        );
        println!("  ✓ Large message: PASS");

        println!("=== All comprehensive verify_message tests passed! ===\n");
    }
}
