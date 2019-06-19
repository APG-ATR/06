use crate::{
    errors::Error,
    util::{EqIgnoreNameAndSpan, EqIgnoreSpan},
};
use std::borrow::Cow;
use swc_common::Spanned;
use swc_ecma_ast::*;

pub(super) trait PatExt {
    fn get_ty(&self) -> Option<&TsType>;
    fn set_ty(&mut self, ty: Option<Box<TsType>>);
}

impl PatExt for Pat {
    fn get_ty(&self) -> Option<&TsType> {
        match *self {
            Pat::Array(ArrayPat { ref type_ann, .. })
            | Pat::Assign(AssignPat { ref type_ann, .. })
            | Pat::Ident(Ident { ref type_ann, .. })
            | Pat::Object(ObjectPat { ref type_ann, .. })
            | Pat::Rest(RestPat { ref type_ann, .. }) => type_ann.as_ref().map(|ty| &*ty.type_ann),

            Pat::Expr(ref pat) => unreachable!("Cannot get type from Pat::Expr\n{:?}", pat),
        }
    }

    fn set_ty(&mut self, ty: Option<Box<TsType>>) {
        match *self {
            Pat::Array(ArrayPat {
                ref mut type_ann, ..
            })
            | Pat::Assign(AssignPat {
                ref mut type_ann, ..
            })
            | Pat::Ident(Ident {
                ref mut type_ann, ..
            })
            | Pat::Object(ObjectPat {
                ref mut type_ann, ..
            })
            | Pat::Rest(RestPat {
                ref mut type_ann, ..
            }) => {
                *type_ann = ty.map(|type_ann| TsTypeAnn {
                    span: type_ann.span(),
                    type_ann,
                })
            }

            Pat::Expr(ref pat) => {
                unreachable!("Cannot set type annottation for expression\n{:?}", pat)
            }
        }
    }
}

pub(super) trait TypeExt<'a>: Into<Cow<'a, TsType>> {
    /// Returns generalized type if `self` is a literal type.
    fn generalize_lit(self) -> Cow<'a, TsType> {
        let ty = self.into();
        match *ty {
            TsType::TsLitType(TsLitType { span, ref lit }) => Cow::Owned(
                TsKeywordType {
                    span,
                    kind: match *lit {
                        TsLit::Bool(Bool { .. }) => TsKeywordTypeKind::TsBooleanKeyword,
                        TsLit::Number(Number { .. }) => TsKeywordTypeKind::TsNumberKeyword,
                        TsLit::Str(Str { .. }) => TsKeywordTypeKind::TsStringKeyword,
                    },
                }
                .into(),
            ),
            _ => ty,
        }
    }
}

impl<'a, T> TypeExt<'a> for T where T: Into<Cow<'a, TsType>> {}

pub(super) trait TypeRefExt {
    /// Returns type annotation.
    fn ann(&self) -> Option<&TsType>;

    fn contains_void(&self) -> bool {
        match self.ann() {
            None => false,
            Some(ref ty) => match **ty {
                TsType::TsKeywordType(TsKeywordType {
                    kind: TsKeywordTypeKind::TsVoidKeyword,
                    ..
                }) => true,

                TsType::TsUnionOrIntersectionType(TsUnionOrIntersectionType::TsUnionType(
                    ref t,
                )) => t.types.iter().any(|t| t.contains_void()),

                TsType::TsThisType(..) => false,
                _ => false,
            },
        }
    }

    fn is_any(&self) -> bool {
        match self.ann() {
            None => true,
            Some(ref ty) => match **ty {
                TsType::TsKeywordType(TsKeywordType {
                    kind: TsKeywordTypeKind::TsAnyKeyword,
                    ..
                }) => true,

                TsType::TsUnionOrIntersectionType(TsUnionOrIntersectionType::TsUnionType(
                    ref t,
                )) => t.types.iter().any(|t| t.is_any()),

                TsType::TsThisType(..) => false,
                _ => false,
            },
        }
    }

    fn is_unknown(&self) -> bool {
        match self.ann() {
            None => true,
            Some(ref ty) => match **ty {
                TsType::TsKeywordType(TsKeywordType {
                    kind: TsKeywordTypeKind::TsUnknownKeyword,
                    ..
                }) => true,

                TsType::TsUnionOrIntersectionType(TsUnionOrIntersectionType::TsUnionType(
                    ref t,
                )) => t.types.iter().any(|t| t.is_unknown()),

                TsType::TsThisType(..) => false,
                _ => false,
            },
        }
    }

    fn contains_undefined(&self) -> bool {
        match self.ann() {
            None => true,
            Some(ref ty) => match **ty {
                TsType::TsKeywordType(TsKeywordType {
                    kind: TsKeywordTypeKind::TsUndefinedKeyword,
                    ..
                }) => true,

                TsType::TsUnionOrIntersectionType(TsUnionOrIntersectionType::TsUnionType(
                    ref t,
                )) => t.types.iter().any(|t| t.contains_undefined()),

                TsType::TsThisType(..) => false,
                _ => false,
            },
        }
    }

    fn assign_to(&self, to: &TsType) -> Option<Error> {
        let rhs = match self.ann() {
            Some(v) => v,
            None => return None,
        };

        try_assign(to, rhs).map(|err| match err {
            Error::AssignFailed { .. } => err,
            _ => Error::AssignFailed {
                left: to.clone(),
                right: rhs.clone(),
                cause: vec![err],
            },
        })
    }
}

fn try_assign(to: &TsType, rhs: &TsType) -> Option<Error> {
    match *rhs {
        TsType::TsKeywordType(TsKeywordType {
            kind: TsKeywordTypeKind::TsAnyKeyword,
            ..
        }) => return None,

        TsType::TsKeywordType(TsKeywordType {
            kind: TsKeywordTypeKind::TsUnknownKeyword,
            ..
        }) => match *to {
            TsType::TsKeywordType(TsKeywordType {
                kind: TsKeywordTypeKind::TsAnyKeyword,
                ..
            })
            | TsType::TsKeywordType(TsKeywordType {
                kind: TsKeywordTypeKind::TsUnknownKeyword,
                ..
            }) => return None,
            _ => {
                return Some(Error::AssignFailed {
                    left: to.clone(),
                    right: rhs.clone(),
                    cause: vec![],
                })
            }
        },

        TsType::TsUnionOrIntersectionType(TsUnionOrIntersectionType::TsUnionType(
            TsUnionType {
                span, ref types, ..
            },
        )) => {
            let errors = types
                .iter()
                .filter_map(|rhs| try_assign(to, rhs))
                .collect::<Vec<_>>();
            if errors.is_empty() {
                return None;
            }
            return Some(Error::UnionError { span, errors });
        }

        _ => {}
    }

    // TODO(kdy1):
    let span = to.span();

    match *to {
        // let a: any = 'foo'
        TsType::TsKeywordType(TsKeywordType {
            kind: TsKeywordTypeKind::TsAnyKeyword,
            ..
        }) => return None,

        // let a: unknown = undefined
        TsType::TsKeywordType(TsKeywordType {
            kind: TsKeywordTypeKind::TsUnknownKeyword,
            ..
        }) => return None,

        TsType::TsKeywordType(TsKeywordType {
            kind: TsKeywordTypeKind::TsObjectKeyword,
            ..
        }) => {
            // let a: object = {};
            match *rhs {
                TsType::TsTypeLit(..)
                | TsType::TsKeywordType(TsKeywordType {
                    kind: TsKeywordTypeKind::TsNumberKeyword,
                    ..
                })
                | TsType::TsKeywordType(TsKeywordType {
                    kind: TsKeywordTypeKind::TsStringKeyword,
                    ..
                })
                | TsType::TsFnOrConstructorType(..) => return None,
                _ => {}
            }
        }

        TsType::TsLitType(TsLitType { ref lit, .. }) => match *to {
            TsType::TsLitType(TsLitType { lit: ref r_lit, .. }) => {
                if lit.eq_ignore_span(r_lit) {
                    return None;
                }
            }
            // TODO(kdy1): Allow
            //
            // let a: true | false = bool
            _ => {}
        },

        TsType::TsThisType(TsThisType { span }) => return Some(Error::CannotAssingToThis { span }),

        // let a: string | number = 'string';
        TsType::TsUnionOrIntersectionType(TsUnionOrIntersectionType::TsUnionType(
            TsUnionType { ref types, .. },
        )) => {
            let vs = types
                .iter()
                .map(|to| try_assign(&to, rhs))
                .collect::<Vec<_>>();
            if vs.iter().any(Option::is_none) {
                return None;
            }
            return Some(Error::UnionError {
                span,
                errors: vs.into_iter().map(Option::unwrap).collect(),
            });
        }

        TsType::TsUnionOrIntersectionType(TsUnionOrIntersectionType::TsIntersectionType(
            TsIntersectionType { ref types, .. },
        )) => {
            let vs = types
                .iter()
                .map(|to| try_assign(&to, rhs))
                .collect::<Vec<_>>();

            for v in vs {
                if let Some(error) = v {
                    return Some(Error::IntersectionError {
                        span,
                        error: box error,
                    });
                }
            }

            return None;
        }

        TsType::TsArrayType(TsArrayType { ref elem_type, .. }) => match rhs {
            TsType::TsArrayType(TsArrayType {
                elem_type: ref rhs_elem_type,
                ..
            }) => {
                return try_assign(elem_type, rhs_elem_type).map(|cause| Error::AssignFailed {
                    left: to.clone(),
                    right: rhs.clone(),
                    cause: vec![cause],
                })
            }
            _ => {
                return Some(Error::AssignFailed {
                    left: to.clone(),
                    right: rhs.clone(),
                    cause: vec![],
                })
            }
        },

        TsType::TsKeywordType(TsKeywordType { kind, .. }) => {
            match *rhs {
                TsType::TsKeywordType(TsKeywordType { kind: rhs_kind, .. }) if rhs_kind == kind => {
                    return None
                }
                _ => {}
            }

            match kind {
                TsKeywordTypeKind::TsStringKeyword => match *rhs {
                    TsType::TsLitType(TsLitType {
                        lit: TsLit::Str(..),
                        ..
                    }) => return None,

                    _ => {}
                },

                TsKeywordTypeKind::TsNumberKeyword => match *rhs {
                    TsType::TsLitType(TsLitType {
                        lit: TsLit::Number(..),
                        ..
                    }) => return None,

                    _ => {}
                },

                TsKeywordTypeKind::TsBooleanKeyword => match *rhs {
                    TsType::TsLitType(TsLitType {
                        lit: TsLit::Bool(..),
                        ..
                    }) => return None,

                    _ => {}
                },

                _ => {}
            }

            return Some(Error::AssignFailed {
                left: to.clone(),
                right: rhs.clone(),
                cause: vec![],
            });
        }

        TsType::TsTypeLit(TsTypeLit { span, ref members }) => match rhs {
            TsType::TsTypeLit(TsTypeLit {
                members: ref rhs_members,
                ..
            }) => {
                if members
                    .iter()
                    .all(|m| rhs_members.iter().any(|rm| rm.eq_ignore_name_and_span(m)))
                {
                    return None;
                }

                let missing_fields = members
                    .iter()
                    .filter(|m| rhs_members.iter().all(|rm| !rm.eq_ignore_name_and_span(m)))
                    .cloned()
                    .collect();
                return Some(Error::MissingFields {
                    span,
                    fields: missing_fields,
                });
            }
            _ => {}
        },

        _ => {}
    }

    // This is slow (at the time of writing)
    if to.eq_ignore_name_and_span(&rhs) {
        return None;
    }

    unimplemented!("assign: \nLeft: {:?}\nRight: {:?}", to, rhs)
}

impl TypeRefExt for Cow<'_, TsType> {
    fn ann(&self) -> Option<&TsType> {
        Some(&**self)
    }
}

impl TypeRefExt for TsType {
    fn ann(&self) -> Option<&TsType> {
        Some(self)
    }
}

impl TypeRefExt for Option<TsType> {
    fn ann(&self) -> Option<&TsType> {
        self.as_ref()
    }
}

impl TypeRefExt for Option<&'_ TsType> {
    fn ann(&self) -> Option<&TsType> {
        *self
    }
}

impl TypeRefExt for Option<Cow<'_, TsType>> {
    fn ann(&self) -> Option<&TsType> {
        self.as_ref().map(|t| &**t)
    }
}
