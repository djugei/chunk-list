use crate::Chunk;
use core::marker::PhantomData;
use std::mem::MaybeUninit;

/// Not really an index, just accesses the Chunks chained.
/// Contains a pointer to the first Chunk and thats it.
///
/// This is just the "anchor" every interesting per-chunk
/// operation is implemented on the Iterator.
///
/// All operations defined directly on this that need to seek
/// always start at the front for every single operation.
///
/// Does not allocate until elements are actually pushed.
pub struct Anchor<T> {
    start: *mut Chunk<T>,
}

impl<T> Drop for Anchor<T> {
    fn drop(&mut self) {
        let mut ptr = self.start;
        while !ptr.is_null() {
            // only visiting each chunk once, and we own them,
            // therefore no double-frees should happen.
            let b = unsafe { Box::from_raw(ptr) };
            ptr = b.next_hint as *mut _;
        }
    }
}

impl<T> Anchor<T> {
    pub fn new() -> Self {
        Self {
            start: std::ptr::null_mut(),
        }
    }

    /// creates a new Anchor containing an allocated, but empty chunk.
    pub fn new_empty() -> Self {
        let b = Box::new(Chunk::new(MaybeUninit::uninit()));
        Self {
            start: Box::into_raw(b),
        }
    }
}

impl<'a, T> IntoIterator for &'a Anchor<T> {
    type Item = &'a Chunk<T>;
    type IntoIter = AnchorIterator<'a, T>;

    fn into_iter(self) -> <Self as std::iter::IntoIterator>::IntoIter {
        AnchorIterator::new(self)
    }
}

pub struct AnchorIterator<'a, T> {
    // we just keep the index around for lifetime reasons
    _index: PhantomData<&'a Anchor<T>>,
    chunk: *const Chunk<T>,
}

impl<'a, T> AnchorIterator<'a, T> {
    pub fn new(index: &'a Anchor<T>) -> Self {
        Self {
            chunk: index.start,
            _index: Default::default(),
        }
    }
}

impl<'a, T> Iterator for AnchorIterator<'a, T> {
    type Item = &'a Chunk<T>;
    fn next(&mut self) -> Option<&'a Chunk<T>> {
        // this is safe Anchor owns the chunk
        // and  we hold a ref to it so lifetimes work out
        let chunk_ref = unsafe { self.chunk.as_ref() };
        if let Some(chunk) = chunk_ref {
            // inside a Anchor Chunks contain a pointer as their next_hint.
            self.chunk = chunk.next_hint as *const _;
            Some(chunk)
        } else {
            None
        }
    }
}

impl<'a, T> IntoIterator for &'a mut Anchor<T> {
    type Item = &'a mut ChunkMut<T>;
    type IntoIter = AnchorIteratorMut<'a, T>;

    fn into_iter(self) -> <Self as std::iter::IntoIterator>::IntoIter {
        AnchorIteratorMut::new(self)
    }
}

#[repr(transparent)]
pub struct ChunkMut<T> {
    chunk: Chunk<T>,
}

impl<T> ChunkMut<T> {
    /// splits this chunk at the specified position
    /// allocates a new chunk
    /// puts pointer to new chunk in next_hint field of current chunk.
    pub fn split(&mut self, pos: usize) {
        let chunk = Box::new(MaybeUninit::uninit());
        let raw_ptr = Box::into_raw(chunk);
        let id = raw_ptr as usize;

        {
            // this is safe because no one else has a &mut to this
            let ptr_ref = unsafe { raw_ptr.as_mut() }.unwrap();
            self.chunk.split(pos, id, ptr_ref);
        }
        // notice how we don't reconstruct the box, so the value is not being dropped
        // and chunk does not contain a dangling reference.
    }

    pub fn push(&mut self, element: T) {
        if let Some(element) = self.chunk.push(element) {
            self.split(self.chunk.len() - 1);
            // this is safe, we just stored the pointer there, no other &mut to it exist.
            let nextref = unsafe { (self.chunk.next_hint as *mut Chunk<T>).as_mut() }.unwrap();
            // this will only fail if one element is bigger than a whole chunk
            // which would be pointless.
            nextref.push(element).unwrap();
        } else {
            // we are good, the first push worked
        }
    }
}

pub struct AnchorIteratorMut<'a, T> {
    /// we just keep the index around for lifetime reasons
    _index: PhantomData<&'a mut Anchor<T>>,
    /// chunk is always the _current_, i.e. last returned, chunk
    /// this is different from most iterators.
    /// we need that so if the chunk is modified and split
    /// this iterator still catches the newly created chunk
    chunk: Pos<T>,
}

enum Pos<T> {
    Start(*mut Chunk<T>),
    Inner(*mut Chunk<T>),
}

impl<'a, T> AnchorIteratorMut<'a, T> {
    pub fn new(index: &'a mut Anchor<T>) -> Self {
        Self {
            chunk: Pos::Start(index.start),
            _index: Default::default(),
        }
    }
}

impl<'a, T> Iterator for AnchorIteratorMut<'a, T> {
    type Item = &'a mut ChunkMut<T>;
    fn next(&mut self) -> Option<&'a mut ChunkMut<T>> {
        match self.chunk {
            Pos::Start(chunk) => {
                // we will momentarily return this chunk, keep it
                // so we only look up what the next chunk is right before going there
                self.chunk = Pos::Inner(chunk);
                // this is safe Anchor owns the chunk
                // and  we hold a ref to it so lifetimes work out
                unsafe { (chunk as *mut ChunkMut<T>).as_mut() }
            }
            Pos::Inner(chunk) => {
                if chunk.is_null() {
                    None
                } else {
                    // we are doing the deref here to avoid creating two &mut
                    // (one passed out from the last .next() call, one here)
                    // im honestly not sure what this implies for correctness
                    // still violates stacked borrows rule
                    // (understandable, don't want the content of &mut to change)
                    // so i might have to use shared references and Cells
                    // but seeing that this
                    // (mutating stuff that a &mut to exists from a pointer)
                    // does not violate any _currently published_ safety constraints
                    // im just gonna roll with it for now.
                    // wished iterators would let you take &'a self not &self in next
                    let next = unsafe { (*chunk).next_hint as *mut Chunk<T> };
                    self.chunk = Pos::Inner(next);
                    unsafe { (next as *mut ChunkMut<T>).as_mut() }
                }
            }
        }
    }
}

#[test]
fn iter() {
    let a: Anchor<u8> = Anchor::new();
    let mut i = (&a).into_iter();
    assert!(i.next().is_none());
}

#[test]
fn iter_empty() {
    let a: Anchor<u8> = Anchor::new_empty();
    let mut i: AnchorIterator<_> = (&a).into_iter();
    assert!(i.next().is_some());
}

#[test]
fn iter_mut() {
    let mut a: Anchor<u8> = Anchor::new_empty();
    let mut i = (&mut a).into_iter();
    let n = i.next().unwrap();
    n.split(0);
    let n = i.next().unwrap();
    n.split(0);
    assert!(i.next().is_some());
    // keeping the last element around after the next next() call
    // might be causing ub under the stacked borrows proposal with my current
    // implementation. not sure how to go about that yet.
    n.push(3);
    assert!(i.next().is_none());
}