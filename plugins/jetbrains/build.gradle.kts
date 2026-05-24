plugins {
    id("org.jetbrains.kotlin.jvm") version "1.9.20"
    id("org.jetbrains.intellij") version "1.17.0"
    id("com.google.protobuf") version "0.9.4"
}

group = "com.carpai"
version = "1.1.0-dev"

repositories {
    mavenCentral()
}

dependencies {
    // gRPC client for server communication
    implementation("io.grpc:grpc-netty-shaded:1.60.0")
    implementation("io.grpc:grpc-protobuf:1.60.0")
    implementation("io.grpc:grpc-stub:1.60.0")
    implementation("com.google.protobuf:protobuf-java:3.25.0")
    
    // Kotlin coroutines
    implementation("org.jetbrains.kotlinx:kotlinx-coroutines-core:1.7.3")
    implementation("org.jetbrains.kotlinx:kotlinx-coroutines-grpc:1.7.3")
    
    // JSON serialization
    implementation("com.google.code.gson:gson:2.10.1")
    
    // javax.annotation for @Generated
    compileOnly("javax.annotation:javax.annotation-api:1.3.2")
}

protobuf {
    protoc {
        artifact = "com.google.protobuf:protoc:3.25.0"
    }
    plugins {
        create("grpc") {
            artifact = "io.grpc:protoc-gen-grpc-java:1.60.0"
        }
    }
    generateProtoTasks {
        all {
            plugins {
                id("grpc")
            }
        }
    }
}

intellij {
    version.set("2023.3")
    type.set("IC") // IntelliJ Community
    plugins.set(listOf("com.intellij.java"))
}

tasks {
    buildSearchableOptions {
        enabled = false
    }
    
    patchPluginXml {
        sinceBuild.set("233")
        untilBuild.set("242.*")
    }
}
