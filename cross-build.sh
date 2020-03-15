#!/bin/bash
docker run --rm -it \
	--env BINDGEN_EXTRA_CLANG_ARGS="--sysroot=/usr/arm-linux-gnueabihf -D__ARM_PCS_VFP -mfpu=vfp -mfloat-abi=hard" \
	--env CARGO_HOME=/home/rust/src/cargo_home \
	-v "$(pwd)":/home/rust/src pepyakin/remarkable-oxide-cross-pi \
	cargo build --target=armv7-unknown-linux-gnueabihf --release

