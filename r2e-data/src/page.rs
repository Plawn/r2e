use r2e_core::prelude::Params;
use serde::{Deserialize, Serialize};

/// Pagination parameters, extractable from query params.
#[derive(Debug, Clone, Deserialize, Params)]
pub struct Pageable {
    #[query]
    #[param(default)]
    pub page: u64,
    #[query]
    #[param(default = 20u64)]
    pub size: u64,
    #[query]
    pub sort: Option<String>,
}

fn default_page_size() -> u64 {
    20
}

impl Default for Pageable {
    fn default() -> Self {
        Self {
            page: 0,
            size: 20,
            sort: None,
        }
    }
}

impl Pageable {
    pub fn offset(&self) -> u64 {
        self.page * self.size
    }
}

/// A page of results with pagination metadata.
#[derive(Debug, Clone, Serialize)]
pub struct Page<T> {
    pub content: Vec<T>,
    pub page: u64,
    pub size: u64,
    pub total_elements: u64,
    pub total_pages: u64,
}

impl<T> Page<T> {
    pub fn new(content: Vec<T>, pageable: &Pageable, total_elements: u64) -> Self {
        let total_pages = if pageable.size == 0 {
            0
        } else {
            (total_elements + pageable.size - 1) / pageable.size
        };
        Self {
            content,
            page: pageable.page,
            size: pageable.size,
            total_elements,
            total_pages,
        }
    }
}
