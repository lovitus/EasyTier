plugins {
    id("com.android.application")
}

android {
    namespace = "com.kkrainbow.easytier.policyprobe"
    compileSdk = 34

    defaultConfig {
        applicationId = "com.kkrainbow.easytier.policyprobe"
        minSdk = 24
        targetSdk = 34
        testInstrumentationRunner = "com.kkrainbow.easytier.policyprobe.PolicyProbeInstrumentation"
        versionCode = 1
        versionName = "1.0"
    }

    buildTypes {
        getByName("debug") {
            // run-as must execute the probe command under this independent app UID.
            isDebuggable = true
            isMinifyEnabled = false
        }
    }
}
