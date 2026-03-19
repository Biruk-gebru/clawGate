pub fn matches_path(pattern: $str, path: &str) -> bool {
    match pattern {
        "*" => true,
        p if p == path => true,
        p if p.ends_with("/*") => {
            let prefix = &p[..p.len() - 2];
            path.starts_with(prefix)
        }
        _ => false,
    }
}

pub fn find_route<'a>(patterns: &'a [String], path: &str) -> Option<&'a str>{
    let mut best: Option<&str> = None;
    let mut best_len: usize = 0;

    for p in patterns {
        if matches_path(p, path) {
            let specificity = if p == "*" {
                0
            } else if p.ends_with("/*") {
                p.len() - 2
            } else {
                usize::MAX
            };

            if specificity > best_len || best.is_none() {
                best_len = specificity;
                best = Some(p.as_str());
            }
        }
    }
    best
}