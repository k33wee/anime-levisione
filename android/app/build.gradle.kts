plugins {
    alias(libs.plugins.android.application)
    alias(libs.plugins.kotlin.android)
    alias(libs.plugins.kotlin.compose)
}

val releaseSigning = mapOf(
    "ANDROID_KEYSTORE_FILE" to providers.environmentVariable("ANDROID_KEYSTORE_FILE").orNull,
    "ANDROID_KEYSTORE_PASSWORD" to providers.environmentVariable("ANDROID_KEYSTORE_PASSWORD").orNull,
    "ANDROID_KEY_ALIAS" to providers.environmentVariable("ANDROID_KEY_ALIAS").orNull,
    "ANDROID_KEY_PASSWORD" to providers.environmentVariable("ANDROID_KEY_PASSWORD").orNull,
)
check(releaseSigning.values.all { it.isNullOrBlank() } || releaseSigning.values.all { !it.isNullOrBlank() }) {
    "Release signing requires all four ANDROID_KEYSTORE_FILE/PASSWORD and ANDROID_KEY_ALIAS/PASSWORD variables."
}
val hasReleaseSigning = releaseSigning.values.all { !it.isNullOrBlank() }

android {
    namespace = "dev.keewee.animelevisione"
    compileSdk = 35
    ndkVersion = "27.2.12479018"

    defaultConfig {
        applicationId = "dev.keewee.animelevisione"
        minSdk = 26
        targetSdk = 35
        versionCode = 5
        versionName = "0.2.3"
        testInstrumentationRunner = "androidx.test.runner.AndroidJUnitRunner"
        testInstrumentationRunnerArguments["liveProviders"] =
            providers.gradleProperty("liveProviders").getOrElse("false")
        ndk {
            abiFilters += setOf("arm64-v8a", "armeabi-v7a", "x86_64", "x86")
        }
    }

    signingConfigs {
        if (hasReleaseSigning) {
            create("release") {
                storeFile = file(releaseSigning.getValue("ANDROID_KEYSTORE_FILE")!!)
                storePassword = releaseSigning.getValue("ANDROID_KEYSTORE_PASSWORD")
                keyAlias = releaseSigning.getValue("ANDROID_KEY_ALIAS")
                keyPassword = releaseSigning.getValue("ANDROID_KEY_PASSWORD")
            }
        }
    }
    buildTypes {
        release {
            isMinifyEnabled = false
            signingConfig = signingConfigs.findByName("release")
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
val bundledLicense by tasks.registering(Copy::class) {
    from(rootProject.file("../LICENSE"))
    into(layout.buildDirectory.dir("generated/licenseAssets"))
}
android.sourceSets.getByName("main").assets.srcDir(layout.buildDirectory.dir("generated/licenseAssets"))
// Original artwork for documentation captures; never included in the release APK.
val demoScreenshotAssets by tasks.registering(Sync::class) {
    from(rootProject.file("../docs/screenshots/fixtures")) {
        include("catalog.json", "*.png")
        into("demo")
    }
    into(layout.buildDirectory.dir("generated/demoScreenshotAssets"))
}
android.sourceSets.getByName("androidTest").assets.srcDir(demoScreenshotAssets.map { it.destinationDir })
tasks.named("preBuild") { dependsOn(verifyRustLibraries, bundledLicense) }

val verifyReleaseSigning by tasks.registering {
    doLast {
        check(hasReleaseSigning) { "Release signing is required; configure the ANDROID_KEYSTORE_* and ANDROID_KEY_* variables." }
        check(file(releaseSigning.getValue("ANDROID_KEYSTORE_FILE")!!).isFile) { "Release keystore file does not exist." }
    }
}
if (providers.gradleProperty("requireReleaseSigning").orNull == "true") {
    // Variant tasks appear after this script; attach lazily so the guard always runs.
    tasks.matching { it.name == "preReleaseBuild" }.configureEach { dependsOn(verifyReleaseSigning) }
}

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
    androidTestImplementation(libs.kotlinx.coroutines.test)
}
