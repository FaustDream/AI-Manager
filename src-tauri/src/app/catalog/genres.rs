//! 类型词清一色显示中文：国际源的英文类型在这里翻一次，入库与筛选项共用同一份表。
//!
//! 表只收「同一概念的不同英文写法」，不做机器翻译：翻错一个类型比不翻更难排查。
//! 中文类型（国内源、Bangumi）原样透传，不在表里再列一遍。

/// 英文类型 → 中文。键一律小写匹配。
const GENRES: &[(&str, &str)] = &[
    ("action", "动作"),
    ("adventure", "冒险"),
    ("animation", "动画"),
    ("anime", "动画"),
    ("comedy", "喜剧"),
    ("crime", "犯罪"),
    ("documentary", "纪录片"),
    ("drama", "剧情"),
    ("family", "家庭"),
    ("fantasy", "奇幻"),
    ("history", "历史"),
    ("horror", "恐怖"),
    ("music", "音乐"),
    ("musical", "音乐剧"),
    ("mystery", "悬疑"),
    ("romance", "爱情"),
    ("sci-fi", "科幻"),
    ("science fiction", "科幻"),
    ("thriller", "惊悚"),
    ("war", "战争"),
    ("western", "西部"),
    ("supernatural", "超自然"),
    ("slice of life", "日常"),
    ("school", "校园"),
    ("shounen", "少年"),
    ("shoujo", "少女"),
    ("seinen", "青年"),
    ("josei", "女性"),
    ("mecha", "机战"),
    ("sports", "运动"),
    ("psychological", "心理"),
    ("psychological thriller", "心理惊悚"),
    ("isekai", "异世界"),
    ("mahou shoujo", "魔法少女"),
    ("magic", "魔法"),
    ("military", "军事"),
    ("martial arts", "武侠"),
    ("space", "太空"),
    ("time travel", "时间旅行"),
    ("vampire", "吸血鬼"),
    ("zombie", "丧尸"),
    ("post-apocalyptic", "末世"),
    ("apocalyptic", "末世"),
    ("detective", "侦探"),
    ("police", "警匪"),
    ("medical", "医疗"),
    ("legal", "律政"),
    ("politics", "政治"),
    ("business", "商战"),
    ("food", "美食"),
    ("cooking", "美食"),
    ("game", "游戏"),
    ("video game", "游戏"),
    ("adventure comedy", "冒险喜剧"),
    ("black comedy", "黑色喜剧"),
    ("dark comedy", "黑色喜剧"),
    ("stand-up comedy", "脱口秀"),
    ("talk show", "脱口秀"),
    ("reality", "真人秀"),
    ("reality tv", "真人秀"),
    ("variety", "综艺"),
    ("news", "新闻"),
    ("soap", "肥皂剧"),
    ("telenovela", "肥皂剧"),
    ("anthology", "单元剧"),
    ("biography", "传记"),
    ("biographical", "传记"),
    ("short", "短片"),
    ("adult", "成人"),
    ("erotica", "情色"),
    ("kids", "少儿"),
    ("children", "少儿"),
    ("teen", "青少年"),
    ("superhero", "超级英雄"),
    ("cyberpunk", "赛博朋克"),
    ("steampunk", "蒸汽朋克"),
    ("dystopian", "反乌托邦"),
    ("survival", "生存"),
    ("disaster", "灾难"),
    ("monster", "怪兽"),
    ("creature", "怪兽"),
    ("mythology", "神话"),
    ("fairy tale", "童话"),
    ("idol", "偶像"),
    ("talent", "选秀"),
    ("audition", "选秀"),
    ("travel", "旅行"),
    ("nature", "自然"),
    ("animals", "动物"),
    ("science", "科学"),
    ("technology", "科技"),
    ("true crime", "真实犯罪"),
    ("biopic", "传记"),
];

/// 命中的英文类型给中文；中文原样返回 `None`，由调用方保留原值。
pub fn localize_genre(value: &str) -> Option<&'static str> {
    let key = value.trim().to_lowercase();
    if key.is_empty() {
        return None;
    }
    GENRES
        .iter()
        .find(|(english, _)| *english == key)
        .map(|(_, chinese)| *chinese)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn english_genres_are_translated_case_insensitively() {
        assert_eq!(localize_genre("Sci-Fi"), Some("科幻"));
        assert_eq!(localize_genre("  drama "), Some("剧情"));
        assert_eq!(localize_genre("SLICE OF LIFE"), Some("日常"));
    }

    #[test]
    fn chinese_and_unknown_values_are_left_alone() {
        assert_eq!(localize_genre("剧情"), None);
        assert_eq!(localize_genre("something new"), None);
        assert_eq!(localize_genre(""), None);
    }
}
