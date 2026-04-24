//session | dialog

use base::dashmap::DashMap;

pub struct Session{
    pub call_map:DashMap<u64,Call>
}

pub enum Call{
    
}