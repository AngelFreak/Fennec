# Source me before Gradle or adb:  source android/env.sh
export ANDROID_HOME="${ANDROID_HOME:-$HOME/Android/Sdk}"
export ANDROID_SDK_ROOT="$ANDROID_HOME"
export JAVA_HOME="${JAVA_HOME:-$HOME/.local/share/jdk-21}"
export PATH="$ANDROID_HOME/platform-tools:$ANDROID_HOME/emulator:$JAVA_HOME/bin:$PATH"
# Fennec's emulator (AVD pixel8-api36 on port 5580); other emulators on this
# machine belong to other projects. Override for a phone:  ANDROID_SERIAL=<serial>
export ANDROID_SERIAL="${ANDROID_SERIAL:-emulator-5580}"
