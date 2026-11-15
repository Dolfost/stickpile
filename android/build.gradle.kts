buildscript {
    dependencies {
        // AGP 9 compiles Kotlin itself (built-in Kotlin) with the Kotlin
        // Gradle plugin on this classpath, 2.2.10 unless a newer one is
        // declared: this is the newer one, the version of the Compose
        // compiler plugin below, which must match it.
        classpath(libs.kotlin.gradle.plugin)
    }
}

plugins {
    alias(libs.plugins.android.application) apply false
    alias(libs.plugins.kotlin.compose) apply false
}
