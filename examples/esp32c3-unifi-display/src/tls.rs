//! TLS 1.3 with an out-of-band SHA-256 pin of the controller's leaf certificate.
//! Pinning binds this endpoint without a wall clock or a public CA store. Both
//! the exact certificate and the handshake's proof of its private key are checked.
use alloc::vec::Vec;
use embedded_tls::{
    Aes128GcmSha256, CertificateEntryRef, CertificateRef, CertificateVerifyRef, CryptoProvider,
    SignatureScheme, TlsError, TlsVerifier,
};
use rand_core::{CryptoRng, RngCore};
use sha2::{Digest, Sha256, Sha384, Sha512};
use x509_cert::der::Decode;

pub struct Provider<R> {
    rng: R,
    verifier: PinnedVerifier,
}
impl<R> Provider<R> {
    pub fn new(rng: R, pin: [u8; 32]) -> Self {
        Self {
            rng,
            verifier: PinnedVerifier {
                pin,
                certificate: Vec::new(),
                transcript: None,
            },
        }
    }
}
impl<R: CryptoRng + RngCore> CryptoProvider for Provider<R> {
    type CipherSuite = Aes128GcmSha256;
    type Signature = Vec<u8>;
    fn rng(&mut self) -> impl embedded_tls::CryptoRngCore {
        &mut self.rng
    }
    fn verifier(&mut self) -> Result<&mut impl TlsVerifier<Aes128GcmSha256>, TlsError> {
        Ok(&mut self.verifier)
    }
}
struct PinnedVerifier {
    pin: [u8; 32],
    certificate: Vec<u8>,
    transcript: Option<Sha256>,
}
impl TlsVerifier<Aes128GcmSha256> for PinnedVerifier {
    // The manually trusted certificate pin identifies the endpoint, including
    // controllers whose factory certificate has no matching IP/DNS SAN.
    fn set_hostname_verification(&mut self, _: &str) -> Result<(), TlsError> {
        Ok(())
    }
    fn verify_certificate(
        &mut self,
        transcript: &Sha256,
        certificate: CertificateRef<'_>,
    ) -> Result<(), TlsError> {
        self.certificate.clear();
        self.transcript = None;
        let Some(CertificateEntryRef::X509(der)) = certificate.entries.first() else {
            return Err(TlsError::InvalidCertificate);
        };
        let digest: [u8; 32] = Sha256::digest(der).into();
        if der.len() > 4096 || digest != self.pin {
            return Err(TlsError::InvalidCertificate);
        }
        self.certificate.extend_from_slice(der);
        self.transcript = Some(transcript.clone());
        Ok(())
    }
    fn verify_signature(&mut self, verify: CertificateVerifyRef<'_>) -> Result<(), TlsError> {
        let transcript = self.transcript.take().ok_or(TlsError::InvalidCertificate)?;
        let mut message = Vec::from([0x20; 64]);
        message.extend_from_slice(b"TLS 1.3, server CertificateVerify\0");
        message.extend_from_slice(&transcript.finalize());
        let certificate = x509_cert::Certificate::from_der(&self.certificate)
            .map_err(|_| TlsError::InvalidCertificate)?;
        let key = certificate
            .tbs_certificate
            .subject_public_key_info
            .subject_public_key
            .as_bytes()
            .ok_or(TlsError::InvalidCertificate)?;
        use rsa::signature::Verifier;
        let valid = match verify.signature_scheme {
            SignatureScheme::EcdsaSecp256r1Sha256 => {
                let key = p256::ecdsa::VerifyingKey::from_sec1_bytes(key)
                    .map_err(|_| TlsError::InvalidCertificate)?;
                let signature = p256::ecdsa::Signature::from_der(verify.signature)
                    .map_err(|_| TlsError::InvalidSignature)?;
                key.verify(&message, &signature).is_ok()
            }
            SignatureScheme::RsaPssRsaeSha256
            | SignatureScheme::RsaPssRsaeSha384
            | SignatureScheme::RsaPssRsaeSha512 => {
                use rsa::pkcs1::DecodeRsaPublicKey;
                let key = rsa::RsaPublicKey::from_pkcs1_der(key)
                    .map_err(|_| TlsError::InvalidCertificate)?;
                let signature = rsa::pss::Signature::try_from(verify.signature)
                    .map_err(|_| TlsError::InvalidSignature)?;
                match verify.signature_scheme {
                    SignatureScheme::RsaPssRsaeSha256 => rsa::pss::VerifyingKey::<Sha256>::new(key)
                        .verify(&message, &signature)
                        .is_ok(),
                    SignatureScheme::RsaPssRsaeSha384 => rsa::pss::VerifyingKey::<Sha384>::new(key)
                        .verify(&message, &signature)
                        .is_ok(),
                    _ => rsa::pss::VerifyingKey::<Sha512>::new(key)
                        .verify(&message, &signature)
                        .is_ok(),
                }
            }
            _ => return Err(TlsError::InvalidSignatureScheme),
        };
        if valid {
            Ok(())
        } else {
            Err(TlsError::InvalidSignature)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn a_matching_pin_still_requires_the_certificate_private_key() {
        let rcgen::CertifiedKey { cert, .. } =
            rcgen::generate_simple_self_signed(alloc::vec!["controller.test".into()]).unwrap();
        let mut certificate = CertificateRef::with_context(&[]);
        certificate
            .add(CertificateEntryRef::X509(cert.der()))
            .unwrap();
        let mut verifier = PinnedVerifier {
            pin: Sha256::digest(cert.der()).into(),
            certificate: Vec::new(),
            transcript: None,
        };
        verifier
            .verify_certificate(&Sha256::new(), certificate)
            .unwrap();
        use rsa::signature::Signer;
        let forged_key = p256::ecdsa::SigningKey::from_bytes((&[5; 32]).into()).unwrap();
        let forged: p256::ecdsa::Signature = forged_key.sign(b"forged handshake");
        assert!(verifier
            .verify_signature(CertificateVerifyRef {
                signature_scheme: SignatureScheme::EcdsaSecp256r1Sha256,
                signature: forged.to_der().as_bytes()
            })
            .is_err());
    }
}
