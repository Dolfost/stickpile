// The Android app of stickpile (docs/android.md). Plugins and libraries come
// from Google's Maven and Maven Central only, the two repositories F-Droid
// accepts for prebuilt artifacts.
pluginManagement {
    repositories {
        google()
        mavenCentral()
    }
}

dependencyResolutionManagement {
    repositoriesMode.set(RepositoriesMode.FAIL_ON_PROJECT_REPOS)
    repositories {
        google()
        mavenCentral()
    }
}

rootProject.name = "stickpile"
include(":app")
