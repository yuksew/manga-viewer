use std::cmp::Ordering;

/// 名前の自然順比較。大小無視・連続数字は数値比較・同値なら先頭ゼロが長い方を後にする。
pub fn natural_cmp(a: &str, b: &str) -> Ordering {
    let (mut a, mut b) = (a.chars().peekable(), b.chars().peekable());
    let mut zero_tie = Ordering::Equal;
    loop {
        match (a.peek().copied(), b.peek().copied()) {
            (None, None) => return zero_tie,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(x), Some(y)) if x.is_ascii_digit() && y.is_ascii_digit() => {
                let da = take_digits(&mut a);
                let db = take_digits(&mut b);
                let (ta, tb) = (da.trim_start_matches('0'), db.trim_start_matches('0'));
                let ord = ta.len().cmp(&tb.len()).then_with(|| ta.cmp(tb));
                if ord != Ordering::Equal {
                    return ord;
                }
                if zero_tie == Ordering::Equal {
                    zero_tie = da.len().cmp(&db.len());
                }
            }
            (Some(x), Some(y)) => {
                a.next();
                b.next();
                let ord = x.to_lowercase().cmp(y.to_lowercase());
                if ord != Ordering::Equal {
                    return ord;
                }
            }
        }
    }
}

fn take_digits(it: &mut std::iter::Peekable<std::str::Chars<'_>>) -> String {
    let mut s = String::new();
    while let Some(&c) = it.peek() {
        if !c.is_ascii_digit() {
            break;
        }
        s.push(c);
        it.next();
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sorted(mut v: Vec<&str>) -> Vec<&str> {
        v.sort_by(|a, b| natural_cmp(a, b));
        v
    }

    #[test]
    fn numbers_compare_by_value() {
        assert_eq!(natural_cmp("2.png", "10.png"), Ordering::Less);
        assert_eq!(natural_cmp("10.png", "2.png"), Ordering::Greater);
        assert_eq!(
            sorted(vec!["10.png", "1.png", "2.png", "100.png"]),
            ["1.png", "2.png", "10.png", "100.png"]
        );
    }

    #[test]
    fn case_is_ignored() {
        assert_eq!(natural_cmp("A.png", "a.png"), Ordering::Equal);
        assert_eq!(natural_cmp("a2.png", "B1.png"), Ordering::Less);
    }

    #[test]
    fn longer_leading_zeros_come_later() {
        assert_eq!(natural_cmp("1.png", "01.png"), Ordering::Less);
        assert_eq!(natural_cmp("001.png", "01.png"), Ordering::Greater);
        // 数値が違えばゼロの数より数値が優先
        assert_eq!(natural_cmp("002.png", "10.png"), Ordering::Less);
    }

    #[test]
    fn paths_and_prefixes() {
        assert_eq!(natural_cmp("a/2.png", "a/10.png"), Ordering::Less);
        assert_eq!(natural_cmp("a", "a1"), Ordering::Less);
        assert_eq!(natural_cmp("第2巻/1.png", "第10巻/1.png"), Ordering::Less);
    }
}
