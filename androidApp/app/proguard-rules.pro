# The Rust core is reached through JNA (UniFFI): JNA finds the native functions and
# the structures by reflection, so neither may be renamed or removed.
-keep class com.sun.jna.** { *; }
-keep class * implements com.sun.jna.** { *; }
-keep class uniffi.mobile_api.** { *; }
-dontwarn java.awt.**
