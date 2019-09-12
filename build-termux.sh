#!/bin/bash

if [[ -z $ANDROID_SDK_ROOT ]]; then
	echo Android SDK not found!
	exit
fi

export CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER=$ANDROID_SDK_ROOT/ndk-bundle/toolchains/llvm/prebuilt/linux-x86_64/bin/aarch64-linux-android24-clang
export RUSTFLAGS="-C link-arg=-Wl,-rpath=/data/data/com.termux/files/usr/lib -C link-arg=-Wl,--enable-new-dtags"

cargo build --target=aarch64-linux-android $@
