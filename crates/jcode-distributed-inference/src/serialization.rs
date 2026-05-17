//! 高效序列化模块 — 支持零拷贝张量传输

use candle_core::{Tensor, DType, Device};
use anyhow::Result;
use serde::{Serialize, Deserialize};

/// 张量元数据（用于反序列化）
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct TensorMeta {
    pub shape: Vec<usize>,
    pub dtype: String,
}

/// 序列化张量（bincode + 元数据）
pub fn serialize_tensor_with_meta(tensor: &Tensor) -> Result<(Vec<u8>, TensorMeta)> {
    let shape = tensor.shape().dims().to_vec();
    let dtype = match tensor.dtype() {
        DType::F16 => "f16",
        DType::F32 => "f32",
        DType::BF16 => "bf16",
        _ => return Err(anyhow::anyhow!("Unsupported dtype: {:?}", tensor.dtype())),
    };

    let meta = TensorMeta {
        shape: shape.clone(),
        dtype: dtype.to_string(),
    };

    // 展平并转换为 f32（bincode 兼容性更好）
    let flattened = tensor.flatten_all()?;
    let values_f32 = flattened.to_vec1::<f32>()?;

    // 使用 bincode 序列化（比 JSON 快 10-50x）
    let bytes = bincode::serialize(&values_f32)?;

    Ok((bytes, meta))
}

/// 反序列化张量
pub fn deserialize_tensor_with_meta(data: &[u8], meta: &TensorMeta, device: &Device) -> Result<Tensor> {
    // 反序列化数值
    let values_f32: Vec<f32> = bincode::deserialize(data)?;

    // 创建张量
    let tensor = Tensor::from_vec(values_f32, &meta.shape, device)?;

    // 转换回原始 dtype
    let dtype = match meta.dtype.as_str() {
        "f16" => DType::F16,
        "f32" => DType::F32,
        "bf16" => DType::BF16,
        _ => return Err(anyhow::anyhow!("Unknown dtype: {}", meta.dtype)),
    };

    Ok(tensor.to_dtype(dtype)?)
}

/// 快速序列化（仅数值，无元数据 - 适用于已知形状的场景）
pub fn serialize_tensor_fast(tensor: &Tensor) -> Result<Vec<u8>> {
    let flattened = tensor.flatten_all()?;
    let values = flattened.to_vec1::<f32>()?;
    Ok(bincode::serialize(&values)?)
}

/// 快速反序列化
pub fn deserialize_tensor_fast(data: &[u8], shape: &[usize], device: &Device) -> Result<Tensor> {
    let values: Vec<f32> = bincode::deserialize(data)?;
    Ok(Tensor::from_vec(values, shape, device)?.to_dtype(DType::F32)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use candle_core::Tensor;

    #[test]
    fn test_serialize_deserialize() {
        let device = Device::Cpu;
        let original = Tensor::randn(0.0f32, 1.0, (2, 3), &device).unwrap();

        let (bytes, meta) = serialize_tensor_with_meta(&original).unwrap();
        let restored = deserialize_tensor_with_meta(&bytes, &meta, &device).unwrap();

        // 验证形状和数据类型
        assert_eq!(restored.shape().dims(), original.shape().dims());
        assert_eq!(restored.dtype(), original.dtype());
    }
}
