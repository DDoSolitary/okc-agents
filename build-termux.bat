@echo off

if "%ANDROID_SDK_ROOT%"=="" (
	echo Android SDK not found!
	exit /b
)

set CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER=%ANDROID_SDK_ROOT%\ndk-bundle\toolchains\llvm\prebuilt\windows-x86_64\bin\aarch64-linux-android24-clang.cmd
set RUSTFLAGS=-C link-arg=-Wl,-rpath=/data/data/com.termux/files/usr/lib -C link-arg=-Wl,--enable-new-dtags

cargo build --target=aarch64-linux-android %*
