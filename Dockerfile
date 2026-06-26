FROM alpine:3.20
COPY target/x86_64-unknown-linux-musl/release/fa3 /usr/local/bin/hpr-skygate
ENTRYPOINT ["hpr-skygate"]
