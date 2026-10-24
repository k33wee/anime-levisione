plugins {
    alias(libs.plugins.android.application)
    alias(libs.plugins.kotlin.android)
    alias(libs.plugins.kotlin.compose)
}

android {
    namespace = "dev.keewee.animelevisione"
    compileSdk = 35
    ndkVersion = "27.2.12479018"

    defaultConfig {
        applicationId = "dev.keewee.animelevisione"
        minSdk = 26
        targetSdk = 35
        versionCode = 2
        versionName = "0.2.0"
        testInstrumentationRunner = "androidx.test.runner.AndroidJUnitRunner"
        testInstrumentationRunnerArguments["liveProviders"] =
            providers.gradleProperty("liveProviders").getOrElse("false")
        ndk {
            abiFilters += setOf("arm64-v8a", "armeabi-v7a", "x86_64", "x86")
        }
    }

    buildTypes {
        release {
            isMinifyEnabled = false
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
    kotlinOptions {
        jvmTarget = "17"
    }
    buildFeatures {
        compose = true
    }
    packaging {
        jniLibs {
            useLegacyPackaging = false
        }
    }
}

// The Kotlin verifier must match the companion crate selected by Cargo.
val rustlsAndroidVersion = providers.fileContents(
    rootProject.layout.projectDirectory.file("../Cargo.lock"),
).asText.map { lock ->
    Regex("""name = "rustls-platform-verifier-android"\r?\nversion = "([^"]+)"""")
        .find(lock)?.groupValues?.get(1)
        ?: error("rustls-platform-verifier-android not found in Cargo.lock")
}

val verifyRustLibraries by tasks.registering {
    val libraries = listOf("arm64-v8a", "armeabi-v7a", "x86_64", "x86").map { abi ->
        layout.projectDirectory.file("src/main/jniLibs/$abi/libanime_levisione_core.so")
    }
    inputs.files(libraries)
    doLast {
        check(libraries.all { it.asFile.isFile }) {
            "Rust Android libraries are missing. Run ./android/build-rust-android.sh from the repository root."
        }
    }
}
tasks.named("preBuild") { dependsOn(verifyRustLibraries) }

dependencies {
    implementation(libs.androidx.activity.compose)
    implementation(platform(libs.androidx.compose.bom))
    implementation(libs.androidx.compose.material3)
    implementation(libs.androidx.compose.ui)
    implementation(libs.androidx.compose.ui.tooling.preview)
    implementation(libs.androidx.lifecycle.runtime.compose)
    implementation(libs.androidx.lifecycle.viewmodel.compose)
    implementation(libs.androidx.compose.material.icons.extended)
    implementation(libs.coil.compose)
    implementation(libs.kotlinx.coroutines.android)
    implementation(libs.media3.exoplayer)
    implementation(libs.media3.exoplayer.hls)
    implementation(libs.media3.ui)
    implementation(libs.media3.database)
    implementation(libs.media3.datasource)
    // The JAR contains only desktop natives; Android needs the AAR's jnidispatch.
    implementation("${libs.jna.get()}@aar")
    implementation(rustlsAndroidVersion.map { "org.rustls:rustls-platform-verifier:$it" })

    androidTestImplementation(platform(libs.androidx.compose.bom))
    androidTestImplementation(libs.androidx.compose.ui.test.junit4)
    androidTestImplementation(libs.androidx.test.runner)
    androidTestImplementation(libs.androidx.test.junit)
}
