// ChessPenguin for Android (docs/design/android-ui.md, docs/research/android-stack.md):
// the interface in Kotlin + Jetpack Compose, the reading of databases in the Rust core
// of the desktop app (crates/mobile-api), bound with UniFFI.
plugins {
    alias(libs.plugins.android.application)
    alias(libs.plugins.compose.compiler)
    alias(libs.plugins.kotlin.serialization)
}

/// The repository root: the Rust workspace, the sample games, the piece sets.
val repo: File = rootDir.parentFile
val cargoOut = layout.buildDirectory.dir("rust/ndk")
val rustLibs = layout.buildDirectory.dir("rust/jniLibs")
val bindings = layout.buildDirectory.dir("generated/uniffi")
val bundled = layout.buildDirectory.dir("generated/assets")
val ndkHome = System.getenv("ANDROID_NDK_HOME")
    ?: "${System.getenv("ANDROID_HOME") ?: "${System.getProperty("user.home")}/Android/Sdk"}/ndk/30.0.16248370"

/// The core for the phones (arm64) and the emulator (x86_64).
val cargoBuild = tasks.register<Exec>("cargoBuild") {
    workingDir = repo
    environment("ANDROID_NDK_HOME", ndkHome)
    commandLine(
        "cargo", "ndk", "-t", "arm64-v8a", "-t", "x86_64", "--platform", "31",
        "-o", cargoOut.get().asFile.path, "build", "--release", "-p", "mobile-api",
    )
    inputs.dir(File(repo, "crates"))
    inputs.file(File(repo, "Cargo.lock"))
    outputs.dir(cargoOut)
}

/// Only our library: cargo-ndk also copies the cdylibs of dependencies (sevenz-rust2),
/// which are linked into ours already.
val rustJniLibs = tasks.register<Copy>("rustJniLibs") {
    dependsOn(cargoBuild)
    from(cargoOut) { include("*/libmobile_api.so") }
    into(rustLibs)
}

/// The Kotlin side of the bridge, generated from the built library.
val uniffiBindings = tasks.register<Exec>("uniffiBindings") {
    dependsOn(rustJniLibs)
    workingDir = repo
    commandLine(
        "cargo", "run", "-q", "-p", "mobile-api", "--features", "bindgen", "--bin", "uniffi-bindgen",
        "--", "generate", "--no-format", "--language", "kotlin",
        "--library", rustLibs.get().file("x86_64/libmobile_api.so").asFile.path,
        "--out-dir", bindings.get().asFile.path,
    )
    inputs.dir(rustLibs)
    outputs.dir(bindings)
}

/// Stockfish for Android (scripts/stockfish.sh): the executable for both builds, its
/// network only inside the "full" one.
val stockfishOut = layout.buildDirectory.dir("stockfish")
val stockfish = tasks.register<Exec>("stockfish") {
    environment("ANDROID_NDK_HOME", ndkHome)
    commandLine(File(rootDir, "scripts/stockfish.sh").path, stockfishOut.get().asFile.path)
    inputs.file(File(rootDir, "scripts/stockfish.sh"))
    outputs.dir(stockfishOut)
}

/// The sample games of the desktop app and the Merida pieces, as app assets.
val bundleAssets = tasks.register<Copy>("bundleAssets") {
    from(File(repo, "examples")) {
        include("*.pgn")
        into("examples")
    }
    from(File(repo, "apps/desktop/assets/pieces/merida")) {
        include("*.svg")
        into("pieces/merida")
    }
    from(stockfishOut.map { it.dir("source") }) {
        into("licenses/stockfish")
    }
    dependsOn(stockfish)
    into(bundled)
}

android {
    namespace = "io.github.vrubelroman.chesspenguin"
    compileSdk = 37
    flavorDimensions += "engine"
    productFlavors {
        // «Stockfish уже внутри»: the network is in the APK, nothing to download.
        create("full") {
            dimension = "engine"
            buildConfigField("boolean", "NET_BUILT_IN", "true")
        }
        // «Stockfish скачивается в приложении»: the network is downloaded on request.
        create("light") {
            dimension = "engine"
            buildConfigField("boolean", "NET_BUILT_IN", "false")
        }
    }

    defaultConfig {
        applicationId = "io.github.vrubelroman.chesspenguin"
        minSdk = 31
        targetSdk = 36
        versionCode = 1
        versionName = "0.1.0"
        ndk {
            abiFilters += listOf("arm64-v8a", "x86_64")
        }
        // The network of Stockfish 19 (scripts/stockfish.sh), checked after download.
        buildConfigField("String", "NET_NAME", "\"nn-1a298aa575a0.nnue\"")
        buildConfigField("String", "NET_SHA256", "\"1a298aa575a085434d29027978dc36867fe9c5bcea9376654b7a8eba1e52dfc2\"")
        buildConfigField("long", "NET_SIZE", "98511183L")
        buildConfigField(
            "String",
            "NET_URL",
            "\"https://github.com/official-stockfish/networks/raw/master/nn-1a298aa575a0.nnue\"",
        )
    }

    buildTypes {
        release {
            isMinifyEnabled = false
            proguardFiles(getDefaultProguardFile("proguard-android-optimize.txt"), "proguard-rules.pro")
        }
    }
    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
    buildFeatures {
        compose = true
        aidl = false
        buildConfig = true
        shaders = false
    }
    sourceSets {
        getByName("main") {
            jniLibs.directories.add(rustLibs.get().asFile.path)
            jniLibs.directories.add(stockfishOut.get().dir("jniLibs").asFile.path)
            kotlin.directories.add(bindings.get().asFile.path)
            assets.directories.add(bundled.get().asFile.path)
        }
        getByName("full") {
            assets.directories.add(stockfishOut.get().dir("net").asFile.path)
        }
    }
    packaging {
        // Unpacked at install: Stockfish runs from nativeLibraryDir as its own process.
        jniLibs {
            useLegacyPackaging = true
        }
        resources {
            excludes += "/META-INF/{AL2.0,LGPL2.1}"
        }
    }
}

kotlin {
    jvmToolchain(21)
}

tasks.named("preBuild") {
    dependsOn(uniffiBindings, bundleAssets, stockfish)
}

dependencies {
    val composeBom = platform(libs.androidx.compose.bom)
    implementation(composeBom)

    implementation(libs.androidx.core.ktx)
    implementation(libs.androidx.lifecycle.runtime.ktx)
    implementation(libs.androidx.activity.compose)
    implementation(libs.androidx.lifecycle.runtime.compose)
    implementation(libs.androidx.lifecycle.viewmodel.compose)

    implementation(libs.androidx.compose.ui)
    implementation(libs.androidx.compose.ui.tooling.preview)
    implementation(libs.androidx.compose.material3)
    implementation(libs.androidx.compose.material.icons)
    debugImplementation(libs.androidx.compose.ui.tooling)

    implementation(libs.androidx.navigation3.ui)
    implementation(libs.androidx.navigation3.runtime)
    implementation(libs.androidx.lifecycle.viewmodel.navigation3)
    implementation(libs.androidx.datastore.preferences)
    implementation(libs.kotlinx.serialization.json)

    // The bridge to the Rust core (UniFFI uses JNA), and the SVG pieces.
    implementation("${libs.jna.get()}@aar")
    implementation(libs.androidsvg)

    testImplementation(libs.junit)
}
