//! The network's tensors by name, read from the published checkpoint.

use std::collections::HashMap;

#[derive(Debug, thiserror::Error)]
pub enum WeightsError {
    #[error("cannot read the weights: {0}")]
    Io(std::io::Error),
    #[error("not a Maia checkpoint")]
    Format,
    #[error("the weights lack {0}")]
    Missing(String),
    #[error("{0} has the shape {1:?}, expected {2:?}")]
    Shape(String, Vec<usize>, Vec<usize>),
}

pub struct Tensor {
    pub shape: Vec<usize>,
    pub data: Vec<f32>,
}

pub struct Weights {
    tensors: HashMap<String, Tensor>,
}

impl Weights {
    /// The tensors of a PyTorch checkpoint (`maia3-5m.pt`).
    pub fn parse(bytes: &[u8]) -> Result<Weights, WeightsError> {
        Ok(Weights {
            tensors: crate::checkpoint::read(bytes)?,
        })
    }

    /// The tensor `name` of this shape.
    pub fn get(&self, name: &str, shape: &[usize]) -> Result<Vec<f32>, WeightsError> {
        let t = self
            .tensors
            .get(name)
            .ok_or_else(|| WeightsError::Missing(name.to_owned()))?;
        if t.shape != shape {
            return Err(WeightsError::Shape(
                name.to_owned(),
                t.shape.clone(),
                shape.to_vec(),
            ));
        }
        Ok(t.data.clone())
    }
}
