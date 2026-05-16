#!/usr/bin/env python3
"""
## 任务 1.1: 模型量化下载脚本

使用 llama.cpp 的 convert-hf-to-gguf.py + quantize 工具，
将 HuggingFace 模型下载并量化为 GGUF Q4_K_M 格式。

### 使用方法

```bash
# 安装依赖
pip install huggingface-hub

# 下载并量化 qwen3 72B
python3 scripts/download_quantize.py \
    --model Qwen/Qwen3-72B \
    --quant Q4_K_M \
    --output ./models

# 下载并量化 deepseek r1 32B
python3 scripts/download_quantize.py \
    --model deepseek-ai/DeepSeek-R1-Distill-Qwen-32B \
    --quant Q4_K_M \
    --output ./models

# 下载并量化 glm5 9B
python3 scripts/download_quantize.py \
    --model THUDM/GLM-5-9B \
    --quant Q4_K_M \
    --output ./models
```

### 硬件要求

Q4_K_M 量化所需硬盘空间：
- 72B 模型: 原始 ~144GB → 量化后 ~36GB（需要 ~180GB 临时磁盘空间）
- 32B 模型: 原始 ~64GB → 量化后 ~18GB（需要 ~80GB 临时磁盘空间）
- 9B 模型: 原始 ~18GB → 量化后 ~6GB（需要 ~25GB 临时磁盘空间）

建议在 128G 内存 + 1T 硬盘的台式机上执行此脚本。
"""

import argparse
import os
import subprocess
import sys
from pathlib import Path


# 支持的模型信息
SUPPORTED_MODELS = {
    "Qwen/Qwen3-72B": {
        "name": "qwen3-72b",
        "params": "72B",
        "layers": 80,
        "context": 32768,
        "type": "chat",
    },
    "Qwen/QwQ-32B-Preview": {
        "name": "qwq-32b",
        "params": "32B",
        "layers": 40,
        "context": 32768,
        "type": "chat",
    },
    "deepseek-ai/DeepSeek-R1-Distill-Qwen-32B": {
        "name": "deepseek-r1-32b",
        "params": "32B",
        "layers": 40,
        "context": 16384,
        "type": "code",
    },
    "THUDM/GLM-5-9B": {
        "name": "glm5-9b",
        "params": "9B",
        "layers": 28,
        "context": 8192,
        "type": "chat",
    },
}


def parse_args():
    parser = argparse.ArgumentParser(
        description="下载并量化 HuggingFace 模型为 GGUF 格式"
    )
    parser.add_argument(
        "--model",
        required=True,
        help="HuggingFace 模型 ID，如 Qwen/Qwen3-72B",
    )
    parser.add_argument(
        "--quant",
        default="Q4_K_M",
        choices=["Q4_K_M", "Q4_0", "Q5_K_M", "Q8_0", "Q3_K_M", "Q2_K"],
        help="量化级别 (默认 Q4_K_M)",
    )
    parser.add_argument(
        "--output",
        default="./models",
        help="输出目录 (默认 ./models)",
    )
    parser.add_argument(
        "--llama-cpp-path",
        default=None,
        help="llama.cpp 路径 (默认从 PATH 查找)",
    )
    parser.add_argument(
        "--skip-download",
        action="store_true",
        help="跳过下载，仅做量化（如果已经下载了原始模型）",
    )
    parser.add_argument(
        "--list-models",
        action="store_true",
        help="列出支持的模型",
    )
    return parser.parse_args()


def list_models():
    print("支持的模型列表:\n")
    print(f"{'模型名称':<50} {'参数量':<8} {'类型':<8} {'层数':<6} {'上下文':<8}")
    print("-" * 80)
    for hf_id, info in SUPPORTED_MODELS.items():
        print(f"{hf_id:<50} {info['params']:<8} {info['type']:<8} {info['layers']:<6} {info['context']:<8}")
    print()


def check_prerequisites():
    """检查 llama.cpp 是否可用"""
    # 检查 llama-quantize
    try:
        subprocess.run(
            ["llama-quantize", "--help"],
            capture_output=True,
            check=False,
        )
        return True
    except FileNotFoundError:
        pass

    print("错误: 未找到 llama.cpp 工具。请先安装:")
    print()
    print("  方式 1: 从源码编译")
    print("    git clone https://github.com/ggerganov/llama.cpp.git")
    print("    cd llama.cpp")
    print("    make -j")
    print("    sudo make install")
    print()
    print("  方式 2: 使用预编译二进制")
    print("    # 从 https://github.com/ggerganov/llama.cpp/releases 下载")
    print()
    print("  方式 3: Windows (使用 CMake)")
    print("    git clone https://github.com/ggerganov/llama.cpp.git")
    print("    cd llama.cpp")
    print("    mkdir build && cd build")
    print("    cmake .. -DCMAKE_BUILD_TYPE=Release")
    print("    cmake --build . --config Release")
    print()
    return False


def download_model(hf_model_id: str, output_dir: Path) -> Path:
    """使用 huggingface-hub 下载模型"""
    from huggingface_hub import snapshot_download

    model_name = hf_model_id.replace("/", "_")
    download_path = output_dir / "raw" / model_name

    if download_path.exists():
        print(f"  模型已存在于 {download_path}，跳过下载")
        return download_path

    print(f"  下载到 {download_path}...")
    print(f"  注意: 这可能需要很长时间（70B 模型约 150GB）")

    download_path = Path(
        snapshot_download(
            repo_id=hf_model_id,
            local_dir=download_path,
            local_dir_use_symlinks=False,
            resume_download=True,
            max_workers=4,
        )
    )

    print(f"  下载完成: {download_path}")
    return download_path


def convert_to_gguf(model_path: Path, output_dir: Path):
    """将 HF 模型转换为 GGUF 格式"""
    # 尝试使用 convert-hf-to-gguf.py
    convert_script = None

    # 查找 convert 脚本
    possible_paths = [
        Path("./llama.cpp/convert-hf-to-gguf.py"),
        Path(os.path.expanduser("~/llama.cpp/convert-hf-to-gguf.py")),
        Path("/usr/local/bin/convert-hf-to-gguf.py"),
    ]

    for p in possible_paths:
        if p.exists():
            convert_script = p
            break

    # 也尝试从 PATH 查找
    try:
        result = subprocess.run(
            ["which", "convert-hf-to-gguf.py"],
            capture_output=True,
            text=True,
        )
        if result.returncode == 0:
            convert_script = Path(result.stdout.strip())
    except FileNotFoundError:
        pass

    if convert_script is None or not convert_script.exists():
        print("  未找到 convert-hf-to-gguf.py 脚本")
        print(f"  请确认 llama.cpp 已安装，或指定路径")
        return None

    gguf_path = output_dir / f"{model_path.name}.fp16.gguf"

    print(f"  转换中: {model_path} → {gguf_path}")
    result = subprocess.run(
        [sys.executable, str(convert_script), str(model_path), "--outfile", str(gguf_path)],
        capture_output=True,
        text=True,
    )

    if result.returncode != 0:
        print(f"  转换失败: {result.stderr}")
        return None

    print(f"  转换完成: {gguf_path}")
    return gguf_path


def quantize_model(gguf_path: Path, quant_type: str, output_dir: Path) -> Path:
    """量化 GGUF 模型"""
    output_name = gguf_path.name.replace(".fp16.gguf", f".{quant_type}.gguf")
    output_path = output_dir / output_name

    print(f"  量化中: {gguf_path} → {output_path} (类型: {quant_type})")

    result = subprocess.run(
        ["llama-quantize", str(gguf_path), str(output_path), quant_type],
        capture_output=True,
        text=True,
    )

    if result.returncode != 0:
        print(f"  量化失败: {result.stderr}")
        return None

    # 计算量化后的文件大小
    size_gb = output_path.stat().st_size / (1024**3)
    print(f"  量化完成: {output_path} ({size_gb:.1f} GB)")

    return output_path


def main():
    args = parse_args()

    if args.list_models:
        list_models()
        return

    if args.model not in SUPPORTED_MODELS:
        print(f"错误: 不支持的模型 '{args.model}'")
        print("使用 --list-models 查看支持的模型列表")
        sys.exit(1)

    if not check_prerequisites():
        sys.exit(1)

    model_info = SUPPORTED_MODELS[args.model]
    output_dir = Path(args.output)
    output_dir.mkdir(parents=True, exist_ok=True)

    print(f"\n{'='*60}")
    print(f"开始处理模型: {args.model}")
    print(f"  参数量: {model_info['params']}")
    print(f"  量化级别: {args.quant}")
    print(f"  输出目录: {output_dir}")
    print(f"{'='*60}\n")

    # 步骤 1: 下载模型
    if not args.skip_download:
        print("[1/3] 下载原始模型...")
        model_path = download_model(args.model, output_dir)
    else:
        model_path = output_dir / "raw" / args.model.replace("/", "_")
        if not model_path.exists():
            print(f"错误: 模型路径 {model_path} 不存在，无法跳过下载")
            sys.exit(1)
        print(f"[1/3] 跳过下载，使用: {model_path}")

    # 步骤 2: 转换为 GGUF
    print(f"\n[2/3] 转换为 GGUF FP16 格式...")
    gguf_path = convert_to_gguf(model_path, output_dir)

    if gguf_path is None:
        print("转换失败，尝试直接量化...")
        # 如果转换失败，尝试直接量化（对某些模型格式直接支持）
        output_name = f"{model_info['name']}.{args.quant}.gguf"
        output_path = output_dir / output_name

        result = subprocess.run(
            ["llama-quantize", str(model_path), str(output_path), args.quant],
            capture_output=True,
            text=True,
        )

        if result.returncode != 0:
            print(f"直接量化也失败: {result.stderr}")
            sys.exit(1)

        gguf_path = output_path

    # 步骤 3: 量化
    print(f"\n[3/3] 量化模型 ({args.quant})...")
    quantized_path = quantize_model(gguf_path, args.quant, output_dir)

    if quantized_path is None:
        sys.exit(1)

    # 清理临时文件
    if gguf_path.exists() and gguf_path != quantized_path:
        temp_size = gguf_path.stat().st_size / (1024**3)
        print(f"\n  清理临时文件: {gguf_path} ({temp_size:.1f} GB)")
        gguf_path.unlink()
        print("  清理完成")

    final_size = quantized_path.stat().st_size / (1024**3)
    print(f"\n{'='*60}")
    print(f"✅ 全部完成!")
    print(f"  模型: {args.model}")
    print(f"  量化: {args.quant}")
    print(f"  文件: {quantized_path}")
    print(f"  大小: {final_size:.1f} GB")
    print(f"  启动命令参考:")
    print(f"    llama-server \\")
    print(f"      --model {quantized_path} \\")
    print(f"      --host 0.0.0.0 --port 18000 \\")
    print(f"      --threads {os.cpu_count() or 4} \\")
    print(f"      --ctx-size 4096 --batch-size 512 \\")
    print(f"      --no-mmap --mlock")
    print(f"{'='=60}\n")


if __name__ == "__main__":
    main()
