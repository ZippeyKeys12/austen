use crate::gen_dsl::choice::draws::Draw;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Site {
    pub width: u16,
}

impl Site {
    pub fn new(width: u16) -> Self {
        Self { width }
    }
}

pub trait DrawSource {
    fn draw(&mut self, site: Site) -> Option<Draw>;
}

pub struct Blind<I>(pub I);

impl<I: Iterator<Item = Draw>> DrawSource for Blind<I> {
    fn draw(&mut self, _site: Site) -> Option<Draw> {
        self.0.next()
    }
}

pub(super) struct Then<A, B> {
    a: A,
    b: B,
    handed_over: bool,
}

impl<A, B> Then<A, B> {
    pub(super) fn new(a: A, b: B) -> Self {
        Self {
            a,
            b,
            handed_over: false,
        }
    }
}

impl<A: DrawSource, B: DrawSource> DrawSource for Then<A, B> {
    fn draw(&mut self, site: Site) -> Option<Draw> {
        if !self.handed_over {
            if let Some(d) = self.a.draw(site) {
                return Some(d);
            }
            self.handed_over = true;
        }
        self.b.draw(site)
    }
}
