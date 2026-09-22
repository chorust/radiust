use crate::errors::{CoreError, CoreResult};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Palette {
    pub id: String,
    pub colors: Vec<[u8; 4]>,
}

impl Palette {
    pub fn new(id: impl Into<String>, colors: Vec<[u8; 4]>) -> CoreResult<Self> {
        let id = id.into();
        if id.is_empty() || colors.is_empty() {
            return Err(CoreError::Storage("palette id and colors are required".into()));
        }
        let mut unique = std::collections::HashSet::new();
        if colors.iter().any(|color| !unique.insert(*color)) {
            return Err(CoreError::Storage("palette colors must be unique".into()));
        }
        Ok(Self { id, colors })
    }

    pub fn color(&self, index: usize) -> Option<[u8; 4]> {
        self.colors.get(index).copied()
    }
}
