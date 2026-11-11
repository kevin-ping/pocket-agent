plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
}

android {
    namespace = "com.hermes.pocketagent.phone"
    compileSdk = 34

    defaultConfig {
        applicationId = "com.hermes.pocketagent.phone"
        minSdk = 30 // Pixel 2 tops out at Android 11
        targetSdk = 34
        versionCode = 2
        versionName = "0.3.2"
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
    kotlinOptions { jvmTarget = "17" }
}

dependencies {
    implementation("com.squareup.okhttp3:okhttp:4.12.0")
    // 1.3.x: the last line that builds with compileSdk 34.
    implementation("androidx.camera:camera-camera2:1.3.4")
    implementation("androidx.camera:camera-lifecycle:1.3.4")
    implementation("androidx.camera:camera-view:1.3.4")
    // Reading the pairing QR code (QrScanner.kt): core only, offline.
    implementation("com.google.zxing:core:3.5.3")
}
