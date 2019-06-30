use super::{
    Array, Function, Interface, Intersection, Param, Tuple, Type, TypeElement, TypeLit, TypeRefExt,
    Union,
};
use crate::{
    errors::Error,
    util::{EqIgnoreNameAndSpan, EqIgnoreSpan},
};
use swc_common::Span;
use swc_ecma_ast::*;

impl Type<'_> {
    pub fn assign_to(&self, to: &Type, span: Span) -> Result<(), Error> {
        try_assign(to, self, span).map_err(|err| match err {
            Error::AssignFailed { .. } => err,
            _ => Error::AssignFailed {
                span,
                left: to.to_static(),
                right: self.to_static(),
                cause: vec![err],
            },
        })
    }
}

fn try_assign(to: &Type, rhs: &Type, span: Span) -> Result<(), Error> {
    macro_rules! fail {
        () => {{
            return Err(Error::AssignFailed {
                span,
                left: to.to_static(),
                right: rhs.to_static(),
                cause: vec![],
            });
        }};
    }

    /// Ensure that $ty is valid.
    /// Type::Array / Type::FnOrConstructor / Type::UnionOrIntersection is
    /// considered invalid
    macro_rules! verify {
        ($ty:expr) => {{
            if cfg!(debug_assertions) {
                match $ty {
                    Type::Simple(ref ty) => match **ty {
                        TsType::TsFnOrConstructorType(..)
                        | TsType::TsArrayType(..)
                        | TsType::TsKeywordType(..)
                        | TsType::TsLitType(..)
                        | TsType::TsUnionOrIntersectionType(..)
                        | TsType::TsTypeLit(..)
                        | TsType::TsThisType(..)
                        | TsType::TsTupleType(..)
                        | TsType::TsConditionalType(..)
                        | TsType::TsMappedType(..)
                        | TsType::TsTypeOperator(..) => {
                            unreachable!("this type should be converted to `Type`")
                        }
                        _ => {}
                    },
                    _ => {}
                }
            }
        }};
    }
    verify!(to);
    verify!(rhs);

    macro_rules! handle_type_lit {
        ($members:expr) => {{
            let members = $members;
            match *rhs.normalize() {
                Type::TypeLit(TypeLit {
                    members: ref rhs_members,
                    ..
                }) => {
                    // TODO: Assign property to proerty, instead of checking equality
                    let mut missing_fields = vec![];

                    'members: for m in members.iter() {
                        if let Some(l_key) = m.key() {
                            for rm in rhs_members {
                                if rm.key() == Some(l_key) {
                                    match m {
                                        TypeElement::Property(ref el) => match rm {
                                            TypeElement::Property(ref r_el) => {
                                                try_assign(
                                                    el.type_ann
                                                        .as_ref()
                                                        .unwrap_or(&Type::any(span).owned()),
                                                    r_el.type_ann
                                                        .as_ref()
                                                        .unwrap_or(&Type::any(span).owned()),
                                                    span,
                                                )?;
                                                continue 'members;
                                            }
                                            _ => {}
                                        },

                                        TypeElement::Method(..) => match rm {
                                            TypeElement::Method(..) => unimplemented!(
                                                "assignment: method property in type literal"
                                            ),
                                            _ => {}
                                        },
                                        _ => {}
                                    }
                                }
                            }

                            // No property with `key` found.
                            missing_fields.push(m.clone().into_static());
                        } else {
                            if !rhs_members.iter().any(|rm| rm.eq_ignore_name_and_span(m)) {
                                missing_fields.push(m.clone().into_static());
                            }
                        }
                    }

                    if missing_fields.is_empty() {
                        return Ok(());
                    }
                    return Err(Error::MissingFields {
                        span,
                        fields: missing_fields,
                    });
                }

                Type::Tuple(..) | Type::Array(..) | Type::Lit(..) => fail!(),

                _ => {}
            }
        }};
    }

    match *to.normalize() {
        // let a: any = 'foo'
        Type::Keyword(TsKeywordType {
            kind: TsKeywordTypeKind::TsAnyKeyword,
            ..
        }) => return Ok(()),

        // Anything is assignable to unknown
        Type::Keyword(TsKeywordType {
            kind: TsKeywordTypeKind::TsUnknownKeyword,
            ..
        }) => return Ok(()),

        _ => {}
    }

    match *rhs.normalize() {
        Type::Union(Union {
            ref types, span, ..
        }) => {
            let errors = types
                .iter()
                .filter_map(|rhs| match try_assign(to, rhs, span) {
                    Ok(()) => None,
                    Err(err) => Some(err),
                })
                .collect::<Vec<_>>();
            if errors.is_empty() {
                return Ok(());
            }
            return Err(Error::UnionError { span, errors });
        }

        Type::Keyword(TsKeywordType {
            kind: TsKeywordTypeKind::TsAnyKeyword,
            ..
        }) => return Ok(()),

        // Handle unknown on rhs
        Type::Keyword(TsKeywordType {
            kind: TsKeywordTypeKind::TsUnknownKeyword,
            ..
        }) => {
            if to.is_keyword(TsKeywordTypeKind::TsAnyKeyword)
                || to.is_keyword(TsKeywordTypeKind::TsUndefinedKeyword)
            {
                return Ok(());
            }

            fail!();
        }

        Type::Param(Param {
            ref name,
            ref constraint,
            ..
        }) => {
            //
            match to.normalize() {
                Type::Param(Param {
                    name: ref l_name, ..
                }) => {
                    if name == l_name {
                        return Ok(());
                    }

                    {}
                }

                _ => {}
            }

            match *constraint {
                Some(ref c) => {
                    return try_assign(to, c, span);
                }
                None => match to.normalize() {
                    Type::TypeLit(TypeLit { ref members, .. }) if members.is_empty() => {
                        return Ok(())
                    }
                    _ => {}
                },
            }

            fail!()
        }

        _ => {}
    }

    match *to.normalize() {
        Type::Array(Array { ref elem_type, .. }) => match rhs {
            Type::Array(Array {
                elem_type: ref rhs_elem_type,
                ..
            }) => {
                //
                return try_assign(&elem_type, &rhs_elem_type, span).map_err(|cause| {
                    Error::AssignFailed {
                        span,
                        left: to.to_static(),
                        right: rhs.to_static(),
                        cause: vec![cause],
                    }
                });
            }

            Type::Tuple(Tuple { ref types, .. }) => {
                for ty in types {
                    try_assign(elem_type, ty, span)?;
                }
                return Ok(());
            }
            _ => fail!(),
        },

        // let a: string | number = 'string';
        Type::Union(Union { ref types, .. }) => {
            let vs = types
                .iter()
                .map(|to| try_assign(&to, rhs, span))
                .collect::<Vec<_>>();
            if vs.iter().any(Result::is_ok) {
                return Ok(());
            }
            return Err(Error::UnionError {
                span,
                errors: vs.into_iter().map(Result::unwrap_err).collect(),
            });
        }

        Type::Intersection(Intersection { ref types, .. }) => {
            let vs = types
                .iter()
                .map(|to| try_assign(&to, rhs, span))
                .collect::<Vec<_>>();

            // TODO: Multiple error
            for v in vs {
                if let Err(error) = v {
                    return Err(Error::IntersectionError {
                        span,
                        error: box error,
                    });
                }
            }

            return Ok(());
        }

        Type::Keyword(TsKeywordType {
            kind: TsKeywordTypeKind::TsObjectKeyword,
            ..
        }) => {
            // let a: object = {};
            match *rhs {
                Type::Keyword(TsKeywordType {
                    kind: TsKeywordTypeKind::TsNumberKeyword,
                    ..
                })
                | Type::Keyword(TsKeywordType {
                    kind: TsKeywordTypeKind::TsStringKeyword,
                    ..
                })
                | Type::Function(..)
                | Type::Constructor(..)
                | Type::Enum(..)
                | Type::Class(..)
                | Type::TypeLit(..) => return Ok(()),

                _ => {}
            }
        }

        // Handle same keyword type.
        Type::Keyword(TsKeywordType { kind, .. }) => {
            match *rhs {
                Type::Keyword(TsKeywordType { kind: rhs_kind, .. }) if rhs_kind == kind => {
                    return Ok(())
                }
                _ => {}
            }

            match kind {
                TsKeywordTypeKind::TsStringKeyword => match *rhs {
                    Type::Lit(TsLitType {
                        lit: TsLit::Str(..),
                        ..
                    }) => return Ok(()),

                    _ => {}
                },

                TsKeywordTypeKind::TsNumberKeyword => match *rhs {
                    Type::Lit(TsLitType {
                        lit: TsLit::Number(..),
                        ..
                    }) => return Ok(()),

                    _ => {}
                },

                TsKeywordTypeKind::TsBooleanKeyword => match *rhs {
                    Type::Lit(TsLitType {
                        lit: TsLit::Bool(..),
                        ..
                    }) => return Ok(()),

                    _ => {}
                },

                _ => {}
            }

            fail!()
        }

        Type::Enum(ref e) => {
            //
            match *rhs {
                Type::EnumVariant(ref r) => {
                    if r.enum_name == e.id.sym {
                        return Ok(());
                    }
                }
                _ => {}
            }

            return Err(Error::AssignFailed {
                span,
                left: Type::Enum(e.clone()),
                right: rhs.to_static(),
                cause: vec![],
            });
        }

        Type::EnumVariant(ref l) => match *rhs {
            Type::EnumVariant(ref r) => {
                if l.enum_name == r.enum_name && l.name == r.name {
                    return Ok(());
                }

                fail!()
            }
            _ => fail!(),
        },

        Type::This(TsThisType { span }) => return Err(Error::CannotAssingToThis { span }),

        // TODO: Handle extends
        Type::Interface(Interface { ref body, .. }) => handle_type_lit!(body),

        Type::TypeLit(TypeLit { ref members, .. }) => handle_type_lit!(members),

        Type::Lit(TsLitType { ref lit, .. }) => match *rhs {
            Type::Lit(TsLitType { lit: ref r_lit, .. }) => {
                if lit.eq_ignore_span(r_lit) {
                    return Ok(());
                }

                fail!()
            }
            // TODO: allow
            // let a: true | false = bool
            _ => fail!(),
        },

        Type::Function(Function {
            type_params: None,
            ref params,
            ref ret_ty,
            ..
        }) => {
            // var fnr2: () => any = fnReturn2();
            match *rhs {
                Type::Function(Function {
                    type_params: None,
                    params: ref r_params,
                    ret_ty: ref r_ret_ty,
                    ..
                }) => {
                    try_assign(ret_ty, r_ret_ty, span)?;
                    // TODO: Verify parameter counts

                    return Ok(());
                }
                _ => {}
            }
        }

        Type::Tuple(Tuple { ref types, .. }) => {
            //
            match *rhs.normalize() {
                Type::Tuple(Tuple {
                    types: ref r_types, ..
                }) => {
                    for (l, r) in types.into_iter().zip(r_types) {
                        match try_assign(l, r, span) {
                            // Great
                            Ok(()) => {}
                            Err(err) => {
                                // I don't know why, but
                                //
                                //      var [a, b]: [number, any] = [undefined, undefined];
                                //
                                // is valid typescript.
                                match *r.normalize() {
                                    Type::Keyword(TsKeywordType {
                                        kind: TsKeywordTypeKind::TsUndefinedKeyword,
                                        ..
                                    }) => {}
                                    _ => return Err(err),
                                }
                            }
                        }
                    }

                    return Ok(());
                }
                _ => {}
            }
        }

        Type::Simple(ref s) => match **s {
            TsType::TsTypePredicate(..) => match *rhs.normalize() {
                Type::Keyword(TsKeywordType {
                    kind: TsKeywordTypeKind::TsBooleanKeyword,
                    ..
                })
                | Type::Lit(TsLitType {
                    lit: TsLit::Bool(..),
                    ..
                }) => return Ok(()),
                _ => {}
            },

            _ => {}
        },

        _ => {}
    }

    // This is slow (at the time of writing)
    if to.eq_ignore_name_and_span(&rhs) {
        return Ok(());
    }

    // Some(Error::Unimplemented {
    //     span,
    //     msg: format!("Not implemented yet"),
    // })
    unimplemented!("assign: \nLeft: {:?}\nRight: {:?}", to, rhs)
}
