//! Schema-provider seam for reflected class and struct shapes.

use crate::package::ObjectPath;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StructSchema {
    pub path: ObjectPath,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClassSchema {
    pub path: ObjectPath,
}

pub trait SchemaProvider {
    fn find_struct(&self, path: &ObjectPath) -> Option<&StructSchema>;
    fn find_class(&self, path: &ObjectPath) -> Option<&ClassSchema>;
}
