#[macro_export]
macro_rules! impl_check_empty {
    ($struct:ident, [$($field:ident),*]) => {
        impl $struct {
            pub fn check_empty(&self) -> bool {
                true $(&& self.$field.is_none())*
            }
        }
    }
}

#[macro_export]
macro_rules! impl_open_close {
    ($struct:ident, { $( $field:ident : $type:ty ),* $(,)? }) => {
        impl $struct {
            $(
                paste! {
                    pub fn [<close_$field>](&mut self) -> bool {
                        self.$field = None;
                        self.check_empty()
                    }

                    pub fn [<open_$field>](&mut self, val: $type) -> bool {
                        if self.$field.is_some() {
                            return false;
                        }
                        self.$field = Some(val);
                        true
                    }
                }
            )*
        }
    };
}

#[macro_export]
macro_rules! impl_close {
    ($struct:ident, [$($field:ident),*]) => {
        impl $struct {
            $(
                paste! {
                    pub fn [<close_$field>](&mut self) -> bool {
                        self.$field = None;
                        self.check_empty()
                    }
                }
            )*
        }
    };
}