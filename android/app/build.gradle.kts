import org.jetbrains.kotlin.gradle.dsl.JvmTarget

plugins {
    alias(libs.plugins.android.application)
    alias(libs.plugins.kotlin.compose)
}

// The repository root: the Rust workspace the native library is built from.
val repoRoot: File = rootProject.projectDir.parentFile

// What cargo-ndk builds, for every ABI of the one universal APK.
val rustAbis = listOf("arm64-v8a", "armeabi-v7a", "x86_64")
val minSdkVersion = 26
val rustJniLibs = layout.buildDirectory.dir("rustJniLibs")
val uniffiKotlin = layout.buildDirectory.dir("generated/uniffi/kotlin")

android {
    namespace = "io.github.dolfost.stickpile"
    compileSdk = 37
    // Keep in step with nix/android-shell.nix and F-Droid's recipe.
    ndkVersion = "28.2.13676358"

    defaultConfig {
        applicationId = "io.github.dolfost.stickpile"
        minSdk = minSdkVersion
        targetSdk = 37
        // Literals: F-Droid's update checker reads them from this file.
        // versionName is the workspace version; versionCode is
        // major * 1000000 + minor * 1000 + patch. checkStickpileVersion
        // fails the build when either disagrees with Cargo.toml.
        versionCode = 8003
        versionName = "0.8.3"
        // One universal APK with these ABIs only (JNA ships more).
        ndk {
            abiFilters += rustAbis
        }
    }

    buildTypes {
        release {
            // No R8: JNA and the UniFFI bindings look classes and methods
            // up by name at run time, so shrinking them needs keep rules,
            // and there are none. No signing config either: the release
            // APK is unsigned, and whoever publishes it signs it.
            isMinifyEnabled = false
        }
    }

    buildFeatures {
        compose = true
    }

    // No dependency metadata block in the APK: F-Droid's scanner flags it
    // in a signed APK, and it carries nothing the app needs.
    dependenciesInfo {
        includeInApk = false
        includeInBundle = false
    }

    // lintVital in a release build fetches Google's SDK index; lint runs in
    // CI (lintDebug) instead, so a release build needs no extra network.
    lint {
        checkReleaseBuilds = false
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    sourceSets {
        getByName("main") {
            jniLibs.directories.add(rustJniLibs.get().asFile.path)
            kotlin.directories.add(uniffiKotlin.get().asFile.path)
        }
    }
}

kotlin {
    compilerOptions {
        jvmTarget.set(JvmTarget.JVM_17)
    }
}

// ManifestTest reads these source files: a change to them reruns the tests.
tasks.withType<Test>().configureEach {
    inputs.file("src/main/AndroidManifest.xml").withPathSensitivity(PathSensitivity.RELATIVE)
    inputs.dir("src/main/res/xml").withPathSensitivity(PathSensitivity.RELATIVE)
}

dependencies {
    // UniFFI's Kotlin bindings call the native library through JNA.
    implementation(libs.jna) { artifact { type = "aar" } }
    implementation(libs.kotlinx.coroutines.android)

    // Activities (Compose).
    implementation(platform(libs.androidx.compose.bom))
    implementation(libs.androidx.compose.material3)
    implementation(libs.androidx.compose.material.icons.core)
    implementation(libs.androidx.activity.compose)
    implementation(libs.androidx.lifecycle.runtime.compose)
    implementation(libs.androidx.lifecycle.viewmodel.compose)
    implementation(libs.androidx.core.ktx)

    // The input method (Views).
    implementation(libs.androidx.recyclerview)
    implementation(libs.material)

    // Sync.
    implementation(libs.androidx.work.runtime)

    // Pairing by QR code.
    implementation(libs.zxing.android.embedded)
    implementation(libs.zxing.core)

    testImplementation(libs.junit)
    testImplementation(libs.kotlinx.coroutines.test)
}

// The workspace version, from the root Cargo.toml.
fun workspaceVersion(): String {
    val toml = repoRoot.resolve("Cargo.toml").readText()
    val section = toml.substringAfter("[workspace.package]")
    return Regex("""(?m)^version\s*=\s*"([^"]+)"""").find(section)?.groupValues?.get(1)
        ?: throw GradleException("no version in [workspace.package] of Cargo.toml")
}

val checkStickpileVersion = tasks.register("checkStickpileVersion") {
    description = "Fails when versionName or versionCode disagree with the workspace version."
    val name = android.defaultConfig.versionName
    val code = android.defaultConfig.versionCode
    val cargoToml = repoRoot.resolve("Cargo.toml")
    inputs.file(cargoToml)
    doLast {
        val version = workspaceVersion()
        val parts = version.split(".").map { it.toIntOrNull() ?: throw GradleException("version $version is not x.y.z") }
        val expectedCode = parts[0] * 1_000_000 + parts[1] * 1_000 + parts[2]
        if (name != version || code != expectedCode) {
            throw GradleException(
                "android/app/build.gradle.kts has versionName $name and versionCode $code, " +
                    "but Cargo.toml says $version (versionCode $expectedCode)",
            )
        }
    }
}

// libstickpile_android.so for every ABI, into build/rustJniLibs/<abi>/.
// Always --release --locked: this is what F-Droid builds.
val cargoNdk = tasks.register<Exec>("cargoNdk") {
    description = "Builds the Rust library (crates/stickpile-android) with cargo-ndk."
    workingDir = repoRoot
    val args = mutableListOf("cargo", "ndk")
    rustAbis.forEach { args += listOf("-t", it) }
    args += listOf(
        "--platform", minSdkVersion.toString(),
        "-o", rustJniLibs.get().asFile.path,
        "build", "--release", "--locked", "-p", "stickpile-android",
    )
    commandLine(args)
}

// The Kotlin side of the interface, generated from the built library (the
// interface is read from the library itself; any ABI's copy will do).
val uniffiBindings = tasks.register<Exec>("uniffiBindings") {
    description = "Generates the Kotlin bindings of the Rust library with uniffi-bindgen."
    dependsOn(cargoNdk)
    workingDir = repoRoot
    val out = uniffiKotlin.get().asFile
    doFirst { delete(out) }
    commandLine(
        "cargo", "run", "--release", "--locked", "-p", "stickpile-android",
        "--features", "bindgen", "--bin", "uniffi-bindgen", "--",
        "generate", "--language", "kotlin", "--no-format",
        "--out-dir", out.path,
        rustJniLibs.get().file("arm64-v8a/libstickpile_android.so").asFile.path,
    )
}

tasks.named("preBuild") {
    dependsOn(checkStickpileVersion, uniffiBindings)
}
