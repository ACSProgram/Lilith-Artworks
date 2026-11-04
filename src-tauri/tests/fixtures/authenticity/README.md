# Authenticity regression fixtures

These files are public test material for automated C2PA regression tests. The
certificate and private key must never be used to sign production content.

The ES256 end-entity certificate is issued by the fixture CA and uses the C2PA
signing EKU. `source.jpg` is an intentionally tiny 128 x 128 source image.
`valid-trustmark.jpg` contains the real application claim and TrustMark soft
binding. `tampered-trustmark.jpg` changes one covered image byte so the manifest
remains readable while C2PA validation fails.

`legacy-0.90-trustmark.jpg` is a frozen copy of the format written by c2pa 0.90
and earlier: the soft binding value is a CBOR text string (`1*<id>`) where the
specification requires a byte string, and 0.91 and later reject the whole
manifest as `claim.malformed`. It is the regression sample for
`authenticity::legacy` and **must not be regenerated**. `valid-trustmark.jpg`
follows the current dependency (c2pa 0.91.10) and stores the value as a byte
string; a test asserts that the legacy reader refuses it.

To regenerate the signed files after an intentional format change, run:

```text
cargo test --manifest-path src-tauri/Cargo.toml --lib authenticity::publish::tests::regenerate_c2pa_fixtures -- --ignored --exact
```
