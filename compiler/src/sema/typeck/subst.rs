// src/sema/typeck/subst.rs
use crate::sema::ty::{TypeId, TypeKind, TypeRegistry};
use crate::utils::SymbolId;
use std::collections::HashMap;

pub struct Substituter<'a> {
    pub registry: &'a mut TypeRegistry,
    pub map: &'a HashMap<SymbolId, TypeId>,
}

impl<'a> Substituter<'a> {
    pub fn new(registry: &'a mut TypeRegistry, map: &'a HashMap<SymbolId, TypeId>) -> Self {
        Self { registry, map }
    }

    pub fn substitute(&mut self, ty: TypeId) -> TypeId {
        if ty == TypeId::ERROR || self.map.is_empty() {
            return ty;
        }

        let kind = self.registry.get(ty).clone();

        match kind {
            TypeKind::Primitive(_) | TypeKind::Error | TypeKind::Module(_) => ty,

            // 命中泛型参数，执行替换
            TypeKind::Param(name) => {
                if let Some(&new_ty) = self.map.get(&name) {
                    new_ty
                } else {
                    ty
                }
            }

            TypeKind::Mut(inner) => {
                let new_inner = self.substitute(inner);
                self.registry.intern(TypeKind::Mut(new_inner))
            }
            TypeKind::Pointer(inner) => {
                let new_inner = self.substitute(inner);
                self.registry.intern(TypeKind::Pointer(new_inner))
            }
            TypeKind::VolatilePtr(inner) => {
                let new_inner = self.substitute(inner);
                self.registry.intern(TypeKind::VolatilePtr(new_inner))
            }
            TypeKind::Slice(inner) => {
                let new_inner = self.substitute(inner);
                self.registry.intern(TypeKind::Slice(new_inner))
            }
            TypeKind::Array { elem, len } => {
                let new_elem = self.substitute(elem);
                self.registry.intern(TypeKind::Array {
                    elem: new_elem,
                    len,
                })
            }
            TypeKind::Function {
                params,
                ret,
                is_variadic,
            } => {
                let new_params = params.into_iter().map(|p| self.substitute(p)).collect();
                let new_ret = self.substitute(ret);
                self.registry.intern(TypeKind::Function {
                    params: new_params,
                    ret: new_ret,
                    is_variadic,
                })
            }
            TypeKind::Def(def_id, args) => {
                let new_args = args.into_iter().map(|a| self.substitute(a)).collect();
                self.registry.intern(TypeKind::Def(def_id, new_args))
            }
            TypeKind::Adt(def_id, args) => {
                let new_args = args.into_iter().map(|a| self.substitute(a)).collect();
                self.registry.intern(TypeKind::Adt(def_id, new_args))
            }
            TypeKind::AdtPayload(def_id, args) => {
                let new_args = args.into_iter().map(|a| self.substitute(a)).collect();
                self.registry.intern(TypeKind::AdtPayload(def_id, new_args))
            }
            TypeKind::FnDef(def_id, args) => {
                let new_args = args.into_iter().map(|a| self.substitute(a)).collect();
                self.registry.intern(TypeKind::FnDef(def_id, new_args))
            }
            TypeKind::Alias(name, target) => {
                let new_target = self.substitute(target);
                self.registry.intern(TypeKind::Alias(name, new_target))
            }
            TypeKind::TraitObject(def_id, args) => {
                let new_args = args.into_iter().map(|a| self.substitute(a)).collect();
                self.registry
                    .intern(TypeKind::TraitObject(def_id, new_args))
            }
        }
    }
}
