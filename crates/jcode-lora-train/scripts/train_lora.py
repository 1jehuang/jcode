#!/usr/bin/env python3
"""
Qwen 代码补全 LoRA 训练脚本
- 使用 unsloth 实现高效 LoRA fine-tune
- 输入: jcode 采集的 completion_dataset.jsonl
- 输出: adapter.safetensors + adapter_config.json

用法:
    python scripts/train_lora.py \
        --data_path ./data/completion_dataset.jsonl \
        --output_dir ./lora_adapters/qwen-code-lora \
        --base_model Qwen/Qwen3-7B-Instruct  # 或 Qwen-14B
"""

import json
import os
import sys
from typing import Dict, List
import argparse

def parse_args():
    parser = argparse.ArgumentParser(description="Qwen LoRA for code completion")
    parser.add_argument("--data_path", required=True, help="Path to completion_dataset.jsonl")
    parser.add_argument("--output_dir", default="./lora_output", help="Output directory")
    parser.add_argument("--base_model", default="Qwen/Qwen3-7B-Instruct", help="Base model")
    parser.add_argument("--r", type=int, default=16, help="LoRA rank")
    parser.add_argument("--epochs", type=int, default=3, help="Training epochs")
    parser.add_argument("--batch_size", type=int, default=4, help="Per-device batch size")
    parser.add_argument("--lr", type=float, default=2e-4, help="Learning rate")
    parser.add_argument("--dry_run", action="store_true", help="Only prepare dataset, no training")
    return parser.parse_args()


def prepare_dataset(data_path: str) -> List[Dict]:
    """加载 jcode 采集的编辑数据 → 训练格式"""
    samples = []
    with open(data_path, "r") as f:
        for line in f:
            if line.strip():
                samples.append(json.loads(line))

    formatted = []
    for s in samples:
        # 输入: 光标位置前的代码
        # 输出: 用户接受的补全文本
        if s.get("accepted"):
            formatted.append({
                "instruction": "Complete the code:",
                "input": s["before"],
                "output": s["accepted"],
            })
    return formatted


def main():
    args = parse_args()
    dataset = prepare_dataset(args.data_path)
    print(f"Loaded {len(dataset)} training samples")

    if args.dry_run:
        print(f"First 3 samples:")
        for d in dataset[:3]:
            print(f"  input: {d['input'][:50]}...")
            print(f"  output: {d['output'][:50]}...")
        return

    try:
        from unsloth import FastLanguageModel
        from unsloth import is_bfloat16_supported
        import torch
        from datasets import Dataset
        from trl import SFTTrainer
        from transformers import TrainingArguments
    except ImportError:
        print("Installing dependencies: pip install unsloth datasets trl")
        os.system("pip install unsloth datasets trl")
        from unsloth import FastLanguageModel
        from datasets import Dataset
        from trl import SFTTrainER
        from transformers import TrainingArguments

    # 加载基座模型
    model, tokenizer = FastLanguageModel.from_pretrained(
        model_name=args.base_model,
        max_seq_length=2048,
        dtype=None,
        load_in_4bit=True,
    )

    # 添加 LoRA
    model = FastLanguageModel.get_peft_model(
        model,
        r=args.r,
        target_modules=["q_proj", "k_proj", "v_proj", "o_proj",
                        "gate_proj", "up_proj", "down_proj"],
        lora_alpha=args.lora_alpha if hasattr(args, 'lora_alpha') else 32,
        use_gradient_checkpointing="unsloth",
        random_state=42,
    )

    # 准备数据集
    def format_func(example):
        return tokenizer.apply_chat_template([
            {"role": "user", "content": f"Complete: {example['input']}"},
            {"role": "assistant", "content": example["output"]},
        ], tokenize=False)

    hf_dataset = Dataset.from_list(dataset)
    hf_dataset = hf_dataset.map(lambda x: {"text": format_func(x)})

    # 训练
    trainer = SFTTrainer(
        model=model,
        tokenizer=tokenizer,
        train_dataset=hf_dataset,
        args=TrainingArguments(
            output_dir=args.output_dir,
            per_device_train_batch_size=args.batch_size,
            gradient_accumulation_steps=4,
            num_train_epochs=args.epochs,
            learning_rate=args.lr,
            fp16=not is_bfloat16_supported(),
            bf16=is_bfloat16_supported(),
            logging_steps=10,
            save_steps=100,
            save_total_limit=2,
        ),
    )

    trainer.train()
    model.save_pretrained(args.output_dir)
    tokenizer.save_pretrained(args.output_dir)
    print(f"✅ LoRA adapter saved to {args.output_dir}")
    print(f"   To use with vLLM:")
    print(f"   python -m vllm.entrypoints.openai.api_server \\")
    print(f"       --model {args.base_model} \\")
    print(f"       --enable-lora \\")
    print(f"       --lora-modules lora={args.output_dir}")


if __name__ == "__main__":
    main()
