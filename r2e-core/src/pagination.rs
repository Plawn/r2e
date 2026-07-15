use r2e_macros::Params;
use serde::{Deserialize, Serialize};

/// Zero-based pagination parameters extracted from query parameters.
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
        self.page.saturating_mul(self.size)
    }
}

/// A page of results and its pagination metadata.
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
            total_elements.div_ceil(pageable.size)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calculates_page_metadata() {
        let pageable = Pageable {
            page: 2,
            size: 20,
            sort: None,
        };
        let page = Page::new(vec![1, 2], &pageable, 41);
        assert_eq!(pageable.offset(), 40);
        assert_eq!(page.total_pages, 3);
    }

    #[test]
    fn zero_size_has_no_pages() {
        let pageable = Pageable {
            page: 0,
            size: 0,
            sort: None,
        };
        let page = Page::<()>::new(Vec::new(), &pageable, 10);
        assert_eq!(page.total_pages, 0);
    }
}
