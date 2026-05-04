// Line comment
//! Inner doc
/// Outer doc
/* block comment */

use std::collections::HashMap;

const MAX: u32 = 42;

#[derive(Clone, Debug)]
pub struct Point<'a> {
    pub name: &'a str,
    pub value: f64,
}

fn main() {
    let s = "hello";
    let raw = r#"raw"#;
    let c = 'x';
    let n: i64 = 0xff_u32 as i64;
    let v = vec![1, 2, 3];

    if let Some(x) = v.first() {
        println!("{}", x);
    } else {
        return;
    }

    match n {
        0 => (),
        _ => unreachable!(),
    }
}
