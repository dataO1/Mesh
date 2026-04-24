//! Map the 400 Discogs EffNet genre labels onto a small set of macro-genre
//! buckets used for level-1 community clustering.
//!
//! The Discogs taxonomy uses "Super---Sub" format, e.g. "Electronic---Drum n Bass".
//! We accept the full label and return a macro-genre, handling the few cases
//! where the same sub-genre name appears under two super-genres (e.g.
//! "Hardcore" is either electronic hardcore or hardcore punk depending on
//! super-genre prefix).
//!
//! Ontology (fixed; user-confirmed):
//!   DnB                  — Drum n Bass, Jungle, Halftime, Broken Beat,
//!                          Breakbeat, Breaks, Big Beat, Progressive Breaks
//!   Trance               — Goa, Psy-Trance, Tech Trance, Trance, Hard Trance,
//!                          Progressive Trance
//!   Techno               — Techno, Minimal Techno, Minimal, Hard Techno,
//!                          Dub Techno, Deep Techno, Bleep
//!   House                — House family (all house variants + disco + acid +
//!                          italo/euro/disco offshoots)
//!   Hardcore             — Hardcore, Happy Hardcore, Hardstyle, Gabber,
//!                          Speedcore, Makina, Jumpstyle, Hands Up, Hi NRG,
//!                          Donk, Schranz, Breakcore
//!   Bass                 — Dubstep, Grime, UK/Speed Garage, Bassline, Juke
//!   Industrial           — Industrial, EBM, Power Electronics, Rhythmic Noise
//!   Synth/Wave           — Synth-pop, Synthwave, Vaporwave, Darkwave,
//!                          Coldwave, Post-Punk, New/No Wave, Electroclash,
//!                          standalone Electro, Freestyle, Italodance,
//!                          Eurobeat, Eurodance, Deathrock, Goth Rock
//!   Ambient/Experimental — Ambient, Dark Ambient, Experimental, Drone,
//!                          Downtempo, Chillwave, IDM, Glitch, Abstract,
//!                          Noise (electronic), New Age, Leftfield, Illbient,
//!                          Dungeon Synth, Berlin-School, Sound Collage,
//!                          Chiptune, Beatdown, Tribal, Musique Concrète,
//!                          Trip Hop, Krautrock-adjacent rock
//!   Rock                 — all Rock---* (all metal, punk, grunge, indie,
//!                          classic rock, etc.) except redirects listed above
//!   Pop                  — all Pop---*, Dance-pop, Pop Rock, Pop Punk,
//!                          Power Pop, Brit Pop, Dream Pop
//!   Hip Hop              — all Hip Hop---* (minus Grime/Bass Music → Bass)
//!   Jazz                 — all Jazz---*, Acid Jazz, Future Jazz, Jazzdance
//!   Classical            — all Classical---*, Modern Classical
//!   Blues                — all Blues---*
//!   Reggae               — all Reggae---*
//!   Funk/Soul            — all Funk / Soul---*
//!   Folk/World           — all Folk, World, & Country---*, all Latin---*,
//!                          Neofolk, Folk Rock, Country Rock
//!   Other                — Non-Music, Brass & Military, Children's,
//!                          Stage & Screen, unknowns

/// Return the macro-genre bucket for a Discogs EffNet "Super---Sub" label.
/// Unknown or empty labels return "Other".
pub fn macro_genre_for(full_label: &str) -> &'static str {
    let l = full_label.to_lowercase();

    // ---------- Rock family (handle first; several sub-labels redirect) ----------
    if let Some(sub) = l.strip_prefix("rock---") {
        if matches!(sub,
            "post-punk" | "coldwave" | "new wave" | "no wave"
            | "deathrock" | "goth rock") {
            return "Synth/Wave";
        }
        if sub == "industrial" { return "Industrial"; }
        if matches!(sub,
            "experimental" | "avantgarde" | "ethereal" | "lo-fi"
            | "space rock" | "krautrock" | "post rock") {
            return "Ambient/Experimental";
        }
        if matches!(sub,
            "dream pop" | "pop punk" | "pop rock" | "power pop" | "brit pop") {
            return "Pop";
        }
        if matches!(sub, "neofolk" | "folk rock" | "country rock") {
            return "Folk/World";
        }
        if matches!(sub, "jazz-rock") {
            return "Jazz";
        }
        return "Rock";
    }

    // ---------- Electronic family (the big block) ----------
    if let Some(sub) = l.strip_prefix("electronic---") {
        // DnB family
        if matches!(sub,
            "drum n bass" | "jungle" | "halftime" | "broken beat"
            | "breakbeat" | "breaks" | "big beat" | "progressive breaks") {
            return "DnB";
        }
        // Hardcore family (includes Breakcore)
        if matches!(sub,
            "hardcore" | "happy hardcore" | "breakcore" | "hardstyle"
            | "gabber" | "speedcore" | "makina" | "jumpstyle"
            | "hands up" | "hi nrg" | "donk" | "schranz") {
            return "Hardcore";
        }
        // Trance family (must come before Techno — "tech trance" is trance)
        if matches!(sub,
            "goa trance" | "psy-trance" | "trance" | "tech trance"
            | "hard trance" | "progressive trance") {
            return "Trance";
        }
        // Techno family
        if matches!(sub,
            "techno" | "minimal techno" | "minimal" | "hard techno"
            | "dub techno" | "deep techno" | "bleep") {
            return "Techno";
        }
        // Bass music
        if matches!(sub,
            "dubstep" | "grime" | "uk garage" | "speed garage"
            | "bassline" | "juke") {
            return "Bass";
        }
        // Industrial
        if matches!(sub,
            "industrial" | "ebm" | "power electronics" | "rhythmic noise") {
            return "Industrial";
        }
        // Synth/Wave
        if matches!(sub,
            "synth-pop" | "synthwave" | "vaporwave" | "darkwave"
            | "new wave" | "electroclash" | "electro" | "freestyle"
            | "italodance" | "eurobeat" | "eurodance") {
            return "Synth/Wave";
        }
        // House family (house + disco lineage + acid variants)
        if matches!(sub,
            "house" | "deep house" | "tech house" | "progressive house"
            | "acid house" | "acid" | "electro house" | "garage house"
            | "ghetto house" | "ghetto" | "hard house" | "hip-house"
            | "italo house" | "tribal house" | "tropical house"
            | "disco" | "euro-disco" | "italo-disco" | "nu-disco"
            | "disco polo" | "euro house") {
            return "House";
        }
        // Cross-over subtypes
        if sub == "dance-pop" { return "Pop"; }
        if sub == "hip hop" { return "Hip Hop"; }
        if matches!(sub, "acid jazz" | "future jazz" | "jazzdance") {
            return "Jazz";
        }
        if matches!(sub, "latin" | "neofolk") {
            return "Folk/World";
        }
        if sub == "modern classical" { return "Classical"; }
        // Everything else under Electronic → Ambient/Experimental
        // (ambient, dark ambient, experimental, drone, downtempo, chillwave,
        //  idm, glitch, abstract, noise, new age, leftfield, illbient,
        //  dungeon synth, berlin-school, sound collage, chiptune, beatdown,
        //  musique concrète, tribal, trip hop, etc.)
        return "Ambient/Experimental";
    }

    // ---------- Hip Hop super-genre ----------
    if let Some(sub) = l.strip_prefix("hip hop---") {
        if matches!(sub, "grime" | "bass music") { return "Bass"; }
        if sub == "trip hop" { return "Ambient/Experimental"; }
        return "Hip Hop";
    }

    // ---------- Directly-mapped super-genres ----------
    if l.starts_with("pop---") { return "Pop"; }
    if l.starts_with("jazz---") { return "Jazz"; }
    if l.starts_with("classical---") { return "Classical"; }
    if l.starts_with("blues---") { return "Blues"; }
    if l.starts_with("reggae---") { return "Reggae"; }
    if l.starts_with("funk / soul---") { return "Funk/Soul"; }
    if l.starts_with("folk, world, & country---") { return "Folk/World"; }
    if l.starts_with("latin---") { return "Folk/World"; }

    // Non-Music, Brass & Military, Children's, Stage & Screen, unknowns
    "Other"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dnb_family() {
        assert_eq!(macro_genre_for("Electronic---Drum n Bass"), "DnB");
        assert_eq!(macro_genre_for("Electronic---Jungle"), "DnB");
        assert_eq!(macro_genre_for("Electronic---Halftime"), "DnB");
        assert_eq!(macro_genre_for("Electronic---Breakbeat"), "DnB");
        assert_eq!(macro_genre_for("Electronic---Big Beat"), "DnB");
    }

    #[test]
    fn test_hardcore_incl_breakcore() {
        assert_eq!(macro_genre_for("Electronic---Hardcore"), "Hardcore");
        assert_eq!(macro_genre_for("Electronic---Happy Hardcore"), "Hardcore");
        assert_eq!(macro_genre_for("Electronic---Breakcore"), "Hardcore");
        assert_eq!(macro_genre_for("Electronic---Hardstyle"), "Hardcore");
        assert_eq!(macro_genre_for("Electronic---Makina"), "Hardcore");
    }

    #[test]
    fn test_trance_before_techno() {
        // tech trance contains "techno" substring but must route to Trance
        assert_eq!(macro_genre_for("Electronic---Tech Trance"), "Trance");
        assert_eq!(macro_genre_for("Electronic---Goa Trance"), "Trance");
        assert_eq!(macro_genre_for("Electronic---Psy-Trance"), "Trance");
    }

    #[test]
    fn test_techno_family() {
        assert_eq!(macro_genre_for("Electronic---Techno"), "Techno");
        assert_eq!(macro_genre_for("Electronic---Minimal Techno"), "Techno");
        assert_eq!(macro_genre_for("Electronic---Dub Techno"), "Techno");
    }

    #[test]
    fn test_rock_redirects() {
        // Rock super-genre, but these map elsewhere per user's ontology
        assert_eq!(macro_genre_for("Rock---Post-Punk"), "Synth/Wave");
        assert_eq!(macro_genre_for("Rock---Coldwave"), "Synth/Wave");
        assert_eq!(macro_genre_for("Rock---New Wave"), "Synth/Wave");
        assert_eq!(macro_genre_for("Rock---Industrial"), "Industrial");
        assert_eq!(macro_genre_for("Rock---Folk Rock"), "Folk/World");
        assert_eq!(macro_genre_for("Rock---Pop Rock"), "Pop");
    }

    #[test]
    fn test_rock_generic() {
        // All other rock + metal + punk → Rock macro
        assert_eq!(macro_genre_for("Rock---Alternative Rock"), "Rock");
        assert_eq!(macro_genre_for("Rock---Heavy Metal"), "Rock");
        assert_eq!(macro_genre_for("Rock---Metalcore"), "Rock");
        assert_eq!(macro_genre_for("Rock---Punk"), "Rock");
        assert_eq!(macro_genre_for("Rock---Hardcore"), "Rock"); // hardcore PUNK, not electronic
    }

    #[test]
    fn test_ambient_experimental_fallthrough() {
        // Anything under Electronic that isn't explicitly mapped → Ambient/Experimental
        assert_eq!(macro_genre_for("Electronic---Ambient"), "Ambient/Experimental");
        assert_eq!(macro_genre_for("Electronic---Experimental"), "Ambient/Experimental");
        assert_eq!(macro_genre_for("Electronic---IDM"), "Ambient/Experimental");
        assert_eq!(macro_genre_for("Electronic---Noise"), "Ambient/Experimental");
        assert_eq!(macro_genre_for("Electronic---Chillwave"), "Ambient/Experimental");
        assert_eq!(macro_genre_for("Electronic---Trip Hop"), "Ambient/Experimental");
    }

    #[test]
    fn test_other_super_genres() {
        assert_eq!(macro_genre_for("Pop---K-pop"), "Pop");
        assert_eq!(macro_genre_for("Jazz---Bebop"), "Jazz");
        assert_eq!(macro_genre_for("Classical---Baroque"), "Classical");
        assert_eq!(macro_genre_for("Blues---Delta Blues"), "Blues");
        assert_eq!(macro_genre_for("Reggae---Dub"), "Reggae");
        assert_eq!(macro_genre_for("Funk / Soul---Soul"), "Funk/Soul");
        assert_eq!(macro_genre_for("Folk, World, & Country---Bluegrass"), "Folk/World");
        assert_eq!(macro_genre_for("Latin---Salsa"), "Folk/World");
    }

    #[test]
    fn test_unknown_returns_other() {
        assert_eq!(macro_genre_for(""), "Other");
        assert_eq!(macro_genre_for("Non-Music---Comedy"), "Other");
        assert_eq!(macro_genre_for("Stage & Screen---Soundtrack"), "Other");
        assert_eq!(macro_genre_for("Unknown---Something"), "Other");
    }
}
