import org.jetbrains.changelog.Changelog
import org.jetbrains.changelog.markdownToHTML

plugins {
    id("java")
    id("org.jetbrains.kotlin.jvm") version "1.9.21"
    id("org.jetbrains.intellij") version "1.17.0"
    id("org.jetbrains.changelog") version "2.2.0"
}

group = "com.carpai"
version = "1.0.0"

repositories {
    mavenCentral()
}

dependencies {
    // Ktor for HTTP client (CarpAI server communication)
    implementation("io.ktor:ktor-client-core:2.3.7")
    implementation("io.ktor:ktor-client-cio:2.3.7")
    implementation("io.ktor:ktor-client-websockets:2.3.7")
    implementation("io.ktor:ktor-serialization-kotlinx-json:2.3.7")

    // Kotlin coroutines
    implementation("org.jetbrains.kotlinx:kotlinx-coroutines-core:1.7.3")
    implementation("org.jetbrains.kotlinx:kotlinx-coroutines-swing:1.7.3")

    // Serialization
    implementation("org.jetbrains.kotlinx:kotlinx-serialization-json:1.6.2")

    // LSP4J for Language Server Protocol
    implementation("org.eclipse.lsp4j:org.eclipse.lsp4j:0.21.1")

    // Logging
    implementation("io.github.microutils:kotlin-logging-jvm:3.0.5")
}

intellij {
    version.set("2023.3")
    type.set("IC") // Target IDE Type: IntelliJ IDEA Community

    // Plugin Dependencies
    plugins.set(
        listOf(
            "com.intellij.modules.platform",
            "com.intellij.modules.lang",
            "com.intellij.modules.vcs",
            "org.jetbrains.plugins.yaml"
        )
    )
}

changelog {
    groups.empty()
    repositoryUrl.set("https://github.com/codecargo/CarpAI")
}

tasks {
    withType<JavaCompile> {
        sourceCompatibility = "17"
        targetCompatibility = "17"
    }

    withType<org.jetbrains.kotlin.gradle.tasks.KotlinCompile> {
        kotlinOptions.jvmTarget = "17"
    }

    patchPluginXml {
        version.set(project.version.toString())
        sinceBuild.set("233")
        untilBuild.set("242.*")

        // Extract the <!-- Plugin description --> section from README.md
        pluginDescription.set(
            projectDir.resolve("README.md").readText().lines().run {
                val start = "<!-- Plugin description -->"
                val end = "<!-- Plugin description end -->"

                if (!containsAll(listOf(start, end))) {
                    return@run ""
                }
                subList(indexOf(start) + 1, indexOf(end))
            }.joinToString("\n").let { markdownToHTML(it) }
        )
    }

    signPlugin {
        certificateChain.set(System.getenv("CERTIFICATE_CHAIN"))
        privateKey.set(System.getenv("PRIVATE_KEY"))
        password.set(System.getenv("PRIVATE_KEY_PASSWORD"))
    }

    publishPlugin {
        dependsOn("patchChangelog")
        token.set(System.getenv("PUBLISH_TOKEN"))
        channels.set(listOf("stable"))
    }
}
