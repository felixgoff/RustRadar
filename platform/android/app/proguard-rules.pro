# Add project specific ProGuard rules here.
# You can control the set of applied configuration files using the
# proguardFiles setting in build.gradle.
#
# For more details, see
#   http://developer.android.com/guide/developing/tools/proguard.html

# Rust calls MainActivity.leave() through JNI and implements its native methods
# (crates/app/src/platform/android.rs); R8 sees neither.
-keep class com.felix.rustradar.MainActivity { *; }

# GameActivity's C glue, compiled into librustradar_app.so by android-activity, finds
# com/google/androidgamesdk/GameActivity by name and registers its native methods by name
# (GameActivity_register: onStartNative, onSurfaceCreatedNative, ...), and GameTextInput reads
# InputConnection and State by name. games-activity 4.4.0's AAR keeps only the members it marks
# @Keep and androidx's Insets and WindowInsetsCompat, so a release build (R8 on) could rename the
# native methods and abort at the first frame. Keep the library whole, and every native method's name.
-keep class com.google.androidgamesdk.** { *; }
-keepclasseswithmembernames,includedescriptorclasses class * {
    native <methods>;
}

# Uncomment this to preserve the line number information for
# debugging stack traces.
#-keepattributes SourceFile,LineNumberTable

# If you keep the line number information, uncomment this to
# hide the original source file name.
#-renamesourcefileattribute SourceFile
