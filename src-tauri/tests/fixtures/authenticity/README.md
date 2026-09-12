# Authenticity regression fixtures

These files are public test material for automated C2PA regression tests. The
certificate and private key must never be used to sign production content.

The ES256 end-entity certificate is issued by the fixture CA and uses the C2PA
signing EKU. `source.jpg` is an intentionally tiny 128 x 128 source image.
`valid-trustmark.jpg` contains the real application claim and TrustMark soft
binding. `tampered-trustmark.jpg` changes one covered image byte so the manifest
remains readable while C2PA validation fails.

To regenerate the signed files after an intentional format change, run:

```text
cargo test --manifest-path src-tauri/Cargo.toml --lib authenticity::pipeline::tests::regenerate_c2pa_fixtures -- --ignored --exact
```
