use super::{
    Page, PageAllocatorInner, PageAllocatorSerialization, PageDeltaSerialization, PageInner,
    PageSerialization, ALLOCATED_PAGES,
};
use ic_sys::{PageBytes, PageIndex};
use std::sync::Arc;

// A memory page allocated on the Rust heap.
#[derive(Debug)]
pub struct HeapBasedPage(PageBytes);

impl HeapBasedPage {
    fn new(contents: &PageBytes) -> Self {
        ALLOCATED_PAGES.inc();
        Self(*contents)
    }
}

impl Drop for HeapBasedPage {
    fn drop(&mut self) {
        ALLOCATED_PAGES.dec();
    }
}

impl PageInner for HeapBasedPage {
    type PageAllocatorInner = HeapBasedPageAllocator;

    fn contents<'a>(&'a self, _page_allocator: &'a Self::PageAllocatorInner) -> &'a PageBytes {
        &self.0
    }

    fn copy_from_slice<'a>(
        &'a mut self,
        offset: usize,
        slice: &[u8],
        _page_allocator: &'a Self::PageAllocatorInner,
    ) {
        (self.0[offset..offset + slice.len()]).copy_from_slice(slice);
    }
}

// A trivial allocator that delegates to the default
// Rust heap allocator.
#[derive(Debug, Default)]
pub struct HeapBasedPageAllocator {}

impl PageAllocatorInner for HeapBasedPageAllocator {
    type PageInner = HeapBasedPage;

    // See the comments of the corresponding method in `PageAllocator`.
    fn allocate(
        &self,
        pages: &[(PageIndex, &PageBytes)],
    ) -> Vec<(PageIndex, Page<Self::PageInner>)> {
        pages
            .iter()
            .map(|(page_index, contents)| {
                (*page_index, Page(Arc::new(HeapBasedPage::new(*contents))))
            })
            .collect()
    }

    // See the comments of the corresponding method in `PageAllocator`.
    fn serialize(&self) -> PageAllocatorSerialization {
        PageAllocatorSerialization::Heap
    }

    // See the comments of the corresponding method in `PageAllocator`.
    fn deserialize(serialized_page_allocator: PageAllocatorSerialization) -> Self {
        match serialized_page_allocator {
            PageAllocatorSerialization::Heap => Default::default(),
            PageAllocatorSerialization::Empty | PageAllocatorSerialization::Mmap(..) => {
                // This is really unreachable. See `serialize()`.
                unreachable!("Unexpected serialization of heap-based page allocator.");
            }
        }
    }

    // See the comments of the corresponding method in `PageAllocator`.
    fn serialize_page_delta<'a, I>(&'a self, page_delta: I) -> PageDeltaSerialization
    where
        I: IntoIterator<Item = (PageIndex, &'a Page<Self::PageInner>)>,
    {
        // Copy the contents of all pages.
        let pages = page_delta
            .into_iter()
            .map(|(index, page)| PageSerialization {
                index,
                bytes: *page.0.contents(self),
            })
            .collect();
        PageDeltaSerialization::Heap(pages)
    }

    // See the comments of the corresponding method in `PageAllocator`.
    fn deserialize_page_delta(
        &self,
        page_delta: PageDeltaSerialization,
    ) -> Vec<(PageIndex, Page<Self::PageInner>)> {
        // Allocate all pages on the Rust heap.
        match page_delta {
            PageDeltaSerialization::Heap(page_delta) => page_delta
                .into_iter()
                .map(|page| (page.index, Page(Arc::new(HeapBasedPage(page.bytes)))))
                .collect(),
            PageDeltaSerialization::Empty | PageDeltaSerialization::Mmap { .. } => {
                // This is really unreachable. See `serialize_page_delta()`.
                unreachable!("Unexpected serialization of page-delta in HeapBasedPageAllocator.");
            }
        }
    }
}