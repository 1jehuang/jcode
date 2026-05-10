# ============================================================
# JCode — KylinOS V10 SP1 兼容构建镜像
# 用于在 x86_64 上模拟 KylinOS 环境进行交叉编译验证
# 基于 KylinOS V10 (CentOS 8 兼容层)
# ============================================================

FROM --platform=$BUILDPLATFORM centos:8 AS builder

ARG TARGETARCH
ENV TARGETARCH=${TARGETARCH:-amd64}

# 安装基础工具
RUN dnf install -y epel-release && \
    dnf groupinstall -y "Development Tools" && \
    dnf install -y \
        openssl-devel \
        fontconfig-devel \
        dbus-devel \
        pkg-config \
        curl \
        && dnf clean all

# 安装 Rust
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
ENV PATH="/root/.cargo/bin:${PATH}"

# 添加 Rust 目标
RUN if [ "$TARGETARCH" = "arm64" ]; then \
        rustup target add aarch64-unknown-linux-gnu; \
    fi

WORKDIR /build

# 复制源码
COPY . .

# 编译
RUN cargo build --release && \
    cp target/release/jcode /usr/local/bin/ && \
    cp target/release/jcode-server /usr/local/bin/ && \
    cp target/release/jcode-grpc /usr/local/bin/

# ── 运行时镜像 ──
FROM centos:8 AS runtime

# 复制二进制
COPY --from=builder /usr/local/bin/jcode /usr/local/bin/jcode
COPY --from=builder /usr/local/bin/jcode-server /usr/local/bin/jcode-server
COPY --from=builder /usr/local/bin/jcode-grpc /usr/local/bin/jcode-grpc

# 系统依赖
RUN dnf install -y \
        openssl \
        fontconfig \
        dbus-libs \
        && dnf clean all

EXPOSE 50051

ENTRYPOINT ["jcode"]
CMD ["--help"]
