use crate::stack_detector::{BuildTool, Framework, Language, TechStack};



/// CI 平台
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Platform { GitLabCi, GitHubActions, Jenkins }

impl Platform {
    pub fn filename(&self) -> &'static str {
        match self {
            Self::GitLabCi => ".gitlab-ci.yml",
            Self::GitHubActions => ".github/workflows/ci.yml",
            Self::Jenkins => "Jenkinsfile",
        }
    }
}

/// CI 模板生成器
pub struct CiTemplate;

impl CiTemplate {
    /// 根据技术栈生成 .gitlab-ci.yml
    pub fn gitlab_ci(stack: &TechStack) -> String {
        let image = Self::docker_image(stack);
        let build_cmd = Self::build_command(stack);
        let test_cmd = Self::test_command(stack);
        let lint_cmd = Self::lint_command(stack);

        format!(r#"# jcode-auto-generated .gitlab-ci.yml
# Tech: {lang} / {fw} / {bt}

image: {image}

stages:
  - lint
  - build
  - test
  - package
  - deploy

variables:
  CARGO_HOME: $CI_PROJECT_DIR/.cargo
  NODE_OPTIONS: --max-old-space-size=4096

cache:
  paths:
    - .cargo/
    - node_modules/
    - target/

before_script:
  - {setup}

lint:
  stage: lint
  script:
    - {lint_cmd}
  only:
    - merge_requests
    - main

build:
  stage: build
  script:
    - {build_cmd}
  artifacts:
    paths:
      - target/
    expire_in: 1 hour

test:
  stage: test
  script:
    - {test_cmd}
  coverage: '/^\d+.\d+% coverage/'

package:
  stage: package
  script:
    - {package_cmd}
  artifacts:
    paths:
      - dist/
    expire_in: 1 week
  only:
    - main

deploy:
  stage: deploy
  script:
    - {deploy_cmd}
  environment:
    name: production
  only:
    - main
"#,
            lang = stack.language.as_str(), fw = Self::fw_name(&stack.framework),
            bt = stack.build_tool.as_str(),
            image = image, setup = Self::setup_command(stack),
            lint_cmd = lint_cmd, build_cmd = build_cmd,
            test_cmd = test_cmd, package_cmd = Self::package_command(stack),
            deploy_cmd = Self::deploy_command(stack),
        )
    }

    /// 生成 GitHub Actions 配置
    pub fn github_actions(stack: &TechStack) -> String {
        format!(r#"# jcode-auto-generated GitHub Actions
name: CI
on: [push, pull_request]
jobs:
  ci:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - run: {build}
      - run: {test}
"#, build = Self::build_command(stack), test = Self::test_command(stack))
    }

    /// 生成 Jenkinsfile
    pub fn jenkinsfile(stack: &TechStack) -> String {
        format!(r#"// jcode-auto-generated Jenkinsfile
pipeline {{
    agent any
    stages {{
        stage('Build') {{ steps {{ sh '{build}' }} }}
        stage('Test')  {{ steps {{ sh '{test}'  }} }}
    }}
}}
"#, build = Self::build_command(stack), test = Self::test_command(stack))
    }

    fn docker_image(stack: &TechStack) -> &'static str {
        match stack.language {
            Language::Rust => "rust:latest",
            Language::TypeScript | Language::JavaScript => "node:20-alpine",
            Language::Python => "python:3.12-slim",
            Language::Java => "maven:3-eclipse-temurin-21",
            Language::Kotlin => "gradle:8-jdk21",
            Language::Go => "golang:1.22-alpine",
            Language::Ruby => "ruby:3.2-alpine",
            _ => "ubuntu:22.04",
        }
    }

    fn fw_name(fw: &Framework) -> &str {
        match fw {
            Framework::Axum => "axum", Framework::Actix => "actix", Framework::SpringBoot => "spring-boot",
            Framework::Express => "express", Framework::NestJs => "nestjs", Framework::NextJs => "nextjs",
            Framework::Django => "django", Framework::FastApi => "fastapi", Framework::Gin => "gin",
            Framework::Flutter => "flutter", _ => "generic",
        }
    }

    fn setup_command(stack: &TechStack) -> String {
        match stack.build_tool {
            BuildTool::Cargo => "",
            BuildTool::Npm | BuildTool::Yarn => "npm ci || yarn install --frozen-lockfile",
            BuildTool::Pnpm => "pnpm install --frozen-lockfile",
            BuildTool::Pipenv => "pipenv install --dev",
            _ => "",
        }.to_string()
    }

    fn build_command(stack: &TechStack) -> String {
        match stack.build_tool {
            BuildTool::Cargo => "cargo build --release",
            BuildTool::Maven => "mvn clean package -DskipTests",
            BuildTool::Gradle => "gradle build -x test",
            BuildTool::Npm | BuildTool::Yarn => "npm run build",
            BuildTool::Pnpm => "pnpm build",
            BuildTool::GoMod => "go build -o bin/app .",
            _ => "make build",
        }.to_string()
    }

    fn test_command(stack: &TechStack) -> String {
        match stack.build_tool {
            BuildTool::Cargo => "cargo test --all-features",
            BuildTool::Maven => "mvn test",
            BuildTool::Gradle => "gradle test",
            BuildTool::Npm => "npm test",
            BuildTool::Yarn => "yarn test",
            BuildTool::Pnpm => "pnpm test",
            BuildTool::GoMod => "go test ./...",
            _ => "make test",
        }.to_string()
    }

    fn lint_command(stack: &TechStack) -> String {
        match stack.language {
            Language::Rust => "cargo clippy -- -D warnings",
            Language::TypeScript | Language::JavaScript => "npx eslint src/",
            Language::Python => "ruff check .",
            Language::Go => "golangci-lint run",
            _ => "echo 'no linter configured'",
        }.to_string()
    }

    fn package_command(stack: &TechStack) -> String {
        match stack.build_tool {
            BuildTool::Cargo => "cargo build --release && cp target/release/app dist/",
            BuildTool::Maven => "mvn package",
            BuildTool::Npm | BuildTool::Yarn => "npm run build && cp -r dist/ dist-package/",
            _ => "make package",
        }.to_string()
    }

    fn deploy_command(stack: &TechStack) -> String {
        if stack.has_dockerfile {
            "docker build -t $CI_REGISTRY_IMAGE . && docker push $CI_REGISTRY_IMAGE".into()
        } else {
            "echo 'deploy: add your deployment script here'".into()
        }
    }
}
