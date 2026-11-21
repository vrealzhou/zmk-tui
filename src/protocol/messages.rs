//! Generated ZMK Studio protocol buffer messages (v3 schema).

pub mod zmk {
    pub mod core {
        include!(concat!(env!("OUT_DIR"), "/zmk.core.rs"));
    }
    pub mod keymap {
        include!(concat!(env!("OUT_DIR"), "/zmk.keymap.rs"));
    }
    pub mod behaviors {
        include!(concat!(env!("OUT_DIR"), "/zmk.behaviors.rs"));
    }
    pub mod meta {
        include!(concat!(env!("OUT_DIR"), "/zmk.meta.rs"));
    }
    pub mod studio {
        include!(concat!(env!("OUT_DIR"), "/zmk.studio.rs"));
    }
}
