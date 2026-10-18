//! Web PKI chain/name/time validation, using only RustCrypto and no_std code.
//! The cloud-authenticated console name is the identity; the TCP address stays on LAN.
use rsa::signature::Verifier;
use rustls_pki_types::{
    alg_id, AlgorithmIdentifier, CertificateDer, InvalidSignature, ServerName,
    SignatureVerificationAlgorithm, TrustAnchor, UnixTime,
};
use sha2::{Sha256, Sha384, Sha512};

#[derive(Debug)]
enum Algorithm {
    Rsa256,
    Rsa384,
    Rsa512,
    Ec256,
    Ec384,
}
impl SignatureVerificationAlgorithm for Algorithm {
    fn public_key_alg_id(&self) -> AlgorithmIdentifier {
        match self {
            Self::Ec256 => alg_id::ECDSA_P256,
            Self::Ec384 => alg_id::ECDSA_P384,
            _ => alg_id::RSA_ENCRYPTION,
        }
    }
    fn signature_alg_id(&self) -> AlgorithmIdentifier {
        match self {
            Self::Rsa256 => alg_id::RSA_PKCS1_SHA256,
            Self::Rsa384 => alg_id::RSA_PKCS1_SHA384,
            Self::Rsa512 => alg_id::RSA_PKCS1_SHA512,
            Self::Ec256 => alg_id::ECDSA_SHA256,
            Self::Ec384 => alg_id::ECDSA_SHA384,
        }
    }
    fn verify_signature(
        &self,
        key: &[u8],
        message: &[u8],
        signature: &[u8],
    ) -> Result<(), InvalidSignature> {
        let valid = match self {
            Self::Ec256 => p256::ecdsa::VerifyingKey::from_sec1_bytes(key)
                .map_err(|_| InvalidSignature)?
                .verify(
                    message,
                    &p256::ecdsa::Signature::from_der(signature).map_err(|_| InvalidSignature)?,
                )
                .is_ok(),
            Self::Ec384 => p384::ecdsa::VerifyingKey::from_sec1_bytes(key)
                .map_err(|_| InvalidSignature)?
                .verify(
                    message,
                    &p384::ecdsa::Signature::from_der(signature).map_err(|_| InvalidSignature)?,
                )
                .is_ok(),
            _ => {
                use rsa::{pkcs1::DecodeRsaPublicKey, traits::PublicKeyParts};
                let key = rsa::RsaPublicKey::from_pkcs1_der(key).map_err(|_| InvalidSignature)?;
                if !(2048..=8192).contains(&key.n().bits()) {
                    return Err(InvalidSignature);
                }
                let signature =
                    rsa::pkcs1v15::Signature::try_from(signature).map_err(|_| InvalidSignature)?;
                match self {
                    Self::Rsa256 => rsa::pkcs1v15::VerifyingKey::<Sha256>::new(key)
                        .verify(message, &signature)
                        .is_ok(),
                    Self::Rsa384 => rsa::pkcs1v15::VerifyingKey::<Sha384>::new(key)
                        .verify(message, &signature)
                        .is_ok(),
                    _ => rsa::pkcs1v15::VerifyingKey::<Sha512>::new(key)
                        .verify(message, &signature)
                        .is_ok(),
                }
            }
        };
        if valid {
            Ok(())
        } else {
            Err(InvalidSignature)
        }
    }
}
static ALGORITHMS: &[&dyn SignatureVerificationAlgorithm] = &[
    &Algorithm::Rsa256,
    &Algorithm::Rsa384,
    &Algorithm::Rsa512,
    &Algorithm::Ec256,
    &Algorithm::Ec384,
];

pub fn verify(
    chain: &[CertificateDer<'_>],
    hostname: &str,
    now: u64,
    roots: &[TrustAnchor<'_>],
) -> Result<(), webpki::Error> {
    let leaf = chain.first().ok_or(webpki::Error::BadDer)?;
    let certificate = webpki::EndEntityCert::try_from(leaf)?;
    certificate.verify_for_usage(
        ALGORITHMS,
        roots,
        &chain[1..],
        UnixTime::since_unix_epoch(core::time::Duration::from_secs(now)),
        webpki::KeyUsage::server_auth(),
        None,
        None,
    )?;
    let name = ServerName::try_from(hostname).map_err(|_| webpki::Error::BadDer)?;
    certificate.verify_is_valid_for_subject_name(&name)
}
