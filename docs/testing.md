# Test the release executable

```sh
cargo test --release --locked
```

The Rust integration tests launch the executable, generate private configuration, make real HTTP requests, and exercise startup failures, OAuth, MCP, log filtering, and an existing database fixture.

The full signed-webhook test also needs a TLS receiver on port 443 at an address that passes production callback validation. The commands below give the receiver a public-range address **inside an isolated Docker network**. The compiled Rust test generates the certificate and verifies the signature over the actual request bytes. It covers on-time and deferred answers, disconnects, concurrency, retries across restart, and subscription removal. No Python or test scripts are required.

```sh
docker network create --internal --subnet 93.184.216.64/28 g2-dot-test
docker run --rm -v "$PWD:/work:ro" -w /work \
  -v g2-dot-registry:/usr/local/cargo/registry -v g2-dot-build:/work/target \
  rust:1.92-bookworm cargo test --release --locked --no-run

docker run --rm --network g2-dot-test --ip 93.184.216.66 \
  --add-host dot-callback.test:93.184.216.66 -e G2_TEST_CALLBACK_IP=93.184.216.66 \
  -v "$PWD:/work:ro" -w /work -v g2-dot-registry:/usr/local/cargo/registry \
  -v g2-dot-build:/work/target rust:1.92-bookworm \
  cargo test --offline --release --locked -- --include-ignored

docker network rm g2-dot-test
docker volume rm g2-dot-registry g2-dot-build
```
