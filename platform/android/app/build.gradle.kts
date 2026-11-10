import java.io.FileInputStream
import java.util.Properties
import org.jetbrains.kotlin.gradle.dsl.JvmTarget

plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
}

// The version CI passes (doc 07 §8.1, §13.4): -PversionName=x.y.z -PversionCode=<major·1000000 +
// minor·1000 + patch>, the code the Tauri build used, so an update installs over it.
val appVersionName = providers.gradleProperty("versionName").getOrElse("1.0")
val appVersionCode = providers.gradleProperty("versionCode").getOrElse("1").toInt()

android {
    compileSdk = 37
    namespace = "com.felix.rustradar"
    defaultConfig {
        manifestPlaceholders["usesCleartextTraffic"] = "false"
        applicationId = "com.felix.rustradar"
        minSdk = 24
        targetSdk = 37
        versionCode = appVersionCode
        versionName = appVersionName
    }
    // Release builds are signed with the keystore CI writes from secrets
    // (keystore.properties: storeFile, password, keyAlias); without one they stay unsigned.
    val keystoreFile = rootProject.file("keystore.properties")
    if (keystoreFile.exists()) {
        val keystore = Properties().apply { FileInputStream(keystoreFile).use { load(it) } }
        signingConfigs {
            create("release") {
                storeFile = file(keystore["storeFile"] as String)
                storePassword = keystore["password"] as String
                keyAlias = keystore["keyAlias"] as String
                keyPassword = keystore["password"] as String
            }
        }
    }
    buildTypes {
        getByName("debug") {
            manifestPlaceholders["usesCleartextTraffic"] = "true"
            isDebuggable = true
            isJniDebuggable = true
            isMinifyEnabled = false
            packaging {
                jniLibs.keepDebugSymbols.add("*/arm64-v8a/*.so")
                jniLibs.keepDebugSymbols.add("*/armeabi-v7a/*.so")
                jniLibs.keepDebugSymbols.add("*/x86/*.so")
                jniLibs.keepDebugSymbols.add("*/x86_64/*.so")
            }
        }
        getByName("release") {
            if (keystoreFile.exists()) signingConfig = signingConfigs.getByName("release")
            optimization {
               enable = true
            }
            proguardFiles(
                *fileTree(".") {
                  include("**/*.pro")
                  exclude("build/**")
                }.files.toTypedArray()
            )
        }
    }
    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_1_8
        targetCompatibility = JavaVersion.VERSION_1_8
    }
    buildFeatures {
        buildConfig = true
    }
}

kotlin {
    compilerOptions {
        jvmTarget = JvmTarget.JVM_1_8
    }
}

dependencies {
    // The Java half of winit's android-game-activity: android-activity 0.6 glues exactly this
    // version (its C side is compiled into librustradar_app.so; no prefab).
    implementation("androidx.games:games-activity:4.4.0")
    implementation("androidx.appcompat:appcompat:1.7.1")
    implementation("androidx.activity:activity-ktx:1.10.1")
    implementation("com.google.android.material:material:1.12.0")
    testImplementation("junit:junit:4.13.2")
    androidTestImplementation("androidx.test.ext:junit:1.1.4")
    androidTestImplementation("androidx.test.espresso:espresso-core:3.5.0")
}
