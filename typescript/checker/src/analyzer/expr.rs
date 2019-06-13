use super::{
    export::{pat_to_ts_fn_param, ExportExtra},
    util::TypeExt,
    Analyzer,
};
use crate::{errors::Error, util::EqIgnoreSpan};
use std::borrow::Cow;
use swc_atoms::{js_word, JsWord};
use swc_common::{Span, Spanned, Visit, VisitWith};
use swc_ecma_ast::*;

impl Analyzer<'_, '_> {
    /// TODO(kdy1): type hint (to reduce computation related to array)
    pub(super) fn type_of<'e>(&self, expr: &'e Expr) -> Result<Cow<'e, TsType>, Error> {
        let span = expr.span();

        Ok(match *expr {
            Expr::This(ThisExpr { span }) => Cow::Owned(TsType::TsThisType(TsThisType { span })),

            Expr::Ident(ref i) => {
                if i.sym == js_word!("require") {
                    unreachable!("typeof(require('...'))");
                }

                if let Some(v) = self.resolved_imports.get(&i.sym) {
                    unreachable!(
                        "Analyzer.type_of() should handle resolved imports. But got {}: {:?}",
                        i.sym, v
                    );
                }

                if let Some(ty) = self.find_var_type(&i.sym) {
                    return Ok(Cow::Owned(ty.clone()));
                }

                // We can return TsTypeRef and depend on expand, but we don't do it in the way
                // in the name of performance.
                if let Ok(ty) = self.expand_export_info(&TsEntityName::Ident(i.clone()), None) {
                    return Ok(Cow::Owned(ty));
                }

                if let Some(ty) = super::defaults::default(&i.sym) {
                    return Ok(Cow::Borrowed(ty));
                }

                // unimplemented!(
                //     "typeof(undefined ident: {})\nFile: {}",
                //     i.sym,
                //     self.path.display()
                // )
                return Err(Error::UndefinedSymbol { span: i.span });
            }

            Expr::Array(ArrayLit { ref elems, .. }) => {
                let mut types: Vec<TsType> = vec![];

                for elem in elems {
                    match elem {
                        Some(ExprOrSpread {
                            spread: None,
                            ref expr,
                        }) => {
                            let ty = self.type_of(expr)?.into_owned().generalize_lit();
                            if types.iter().all(|l| !l.eq_ignore_span(&ty)) {
                                types.push(ty)
                            }
                        }
                        Some(ExprOrSpread {
                            spread: Some(..), ..
                        }) => unimplemented!("type of array spread"),
                        None => {
                            let ty = undefined(span);
                            if types.iter().all(|l| !l.eq_ignore_span(&ty)) {
                                types.push(ty.clone())
                            }
                        }
                    }
                }

                Cow::Owned(TsType::TsArrayType(TsArrayType {
                    span,
                    elem_type: match types.len() {
                        0 => box any(span),
                        1 => box types.into_iter().next().unwrap(),
                        _ => box TsType::TsUnionOrIntersectionType(
                            TsUnionOrIntersectionType::TsUnionType(TsUnionType {
                                span,
                                types: types.into_iter().map(Box::new).collect(),
                            }),
                        ),
                    },
                }))
            }

            Expr::Lit(Lit::Bool(v)) => Cow::Owned(TsType::TsLitType(TsLitType {
                span: v.span,
                lit: TsLit::Bool(v),
            })),
            Expr::Lit(Lit::Str(ref v)) => Cow::Owned(TsType::TsLitType(TsLitType {
                span: v.span,
                lit: TsLit::Str(v.clone()),
            })),
            Expr::Lit(Lit::Num(v)) => Cow::Owned(TsType::TsLitType(TsLitType {
                span: v.span,
                lit: TsLit::Number(v),
            })),
            Expr::Lit(Lit::Null(Null { span })) => {
                Cow::Owned(TsType::TsKeywordType(TsKeywordType {
                    span,
                    kind: TsKeywordTypeKind::TsNullKeyword,
                }))
            }
            Expr::Lit(Lit::Regex(..)) => Cow::Owned(TsType::TsTypeRef(TsTypeRef {
                span,
                type_name: TsEntityName::Ident(Ident {
                    span,
                    sym: js_word!("RegExp"),
                    optional: false,
                    type_ann: None,
                }),
                type_params: None,
            })),

            Expr::Paren(ParenExpr { ref expr, .. }) => return self.type_of(expr),

            Expr::Tpl(..) => Cow::Owned(TsType::TsKeywordType(TsKeywordType {
                span,
                kind: TsKeywordTypeKind::TsStringKeyword,
            })),

            Expr::Unary(UnaryExpr {
                op: op!("!"),
                ref arg,
                ..
            }) => negate(self.type_of(arg)?),

            Expr::Unary(UnaryExpr {
                op: op!("typeof"), ..
            }) => Cow::Owned(TsType::TsKeywordType(TsKeywordType {
                span,
                kind: TsKeywordTypeKind::TsStringKeyword,
            })),

            Expr::TsAs(TsAsExpr { ref type_ann, .. }) => Cow::Borrowed(type_ann),
            Expr::TsTypeCast(TsTypeCastExpr { ref type_ann, .. }) => {
                Cow::Borrowed(&*type_ann.type_ann)
            }

            Expr::TsNonNull(TsNonNullExpr { ref expr, .. }) => {
                return self
                    .type_of(expr)
                    .map(|ty| {
                        // TODO: Optimize

                        ty.into_owned().remove_falsy()
                    })
                    .map(Cow::Owned);
            }

            Expr::Object(ObjectLit { span, ref props }) => {
                Cow::Owned(TsType::TsTypeLit(TsTypeLit {
                    span,
                    members: props
                        .iter()
                        .map(|prop| match *prop {
                            PropOrSpread::Prop(ref prop) => self.type_of_prop(&prop),
                            PropOrSpread::Spread(..) => {
                                unimplemented!("spread element in object literal")
                            }
                        })
                        .collect(),
                }))
            }

            // https://github.com/Microsoft/TypeScript/issues/26959
            Expr::Yield(..) => Cow::Owned(any(span)),

            Expr::Update(..) => Cow::Owned(TsType::TsKeywordType(TsKeywordType {
                kind: TsKeywordTypeKind::TsNumberKeyword,
                span,
            })),

            Expr::Cond(CondExpr {
                ref cons, ref alt, ..
            }) => {
                let cons_ty = self.type_of(cons)?;
                let alt_ty = self.type_of(alt)?;
                if cons_ty.eq_ignore_span(&alt_ty) {
                    cons_ty
                } else {
                    Cow::Owned(TsType::TsUnionOrIntersectionType(
                        TsUnionOrIntersectionType::TsUnionType(TsUnionType {
                            span,
                            types: vec![box cons_ty.into_owned(), box alt_ty.into_owned()],
                        }),
                    ))
                }
            }

            Expr::New(NewExpr {
                ref callee,
                ref type_args,
                ref args,
                ..
            }) => {
                let callee_type = self
                    .extract_call_new_expr(
                        callee,
                        ExtractKind::New,
                        args.as_ref().map(|v| &**v).unwrap_or_else(|| &[]),
                        type_args.as_ref(),
                    )?
                    .into_owned();
                return Ok(Cow::Owned(callee_type));
            }

            Expr::Call(CallExpr {
                callee: ExprOrSuper::Expr(ref callee),
                ref args,
                ref type_args,
                ..
            }) => {
                let callee_type = self
                    .extract_call_new_expr(callee, ExtractKind::Call, args, type_args.as_ref())
                    .map(|v| v.into_owned())?;

                return Ok(Cow::Owned(callee_type));
            }

            // super() returns any
            Expr::Call(CallExpr {
                callee: ExprOrSuper::Super(..),
                ..
            }) => Cow::Owned(any(span)),

            Expr::Seq(SeqExpr { ref exprs, .. }) => {
                assert!(exprs.len() >= 1);

                return self.type_of(&exprs.last().unwrap());
            }

            Expr::Await(AwaitExpr { .. }) => unimplemented!("typeof(AwaitExpr)"),

            Expr::Class(ClassExpr { ref class, .. }) => {
                return self.type_of_class(class).map(Cow::Owned)
            }

            Expr::Arrow(ArrowExpr { .. }) => unimplemented!("typeof(ArrowExpr)"),

            Expr::Fn(FnExpr { ref function, .. }) => {
                return self.type_of_fn(&function).map(Cow::Owned)
            }

            Expr::Member(MemberExpr {
                obj: ExprOrSuper::Expr(ref obj),
                computed,
                ref prop,
                ..
            }) => {
                // member expression
                let obj_ty = self
                    .type_of(obj)
                    .map(Cow::into_owned)
                    .map(Box::new)
                    .map(|obj_type| {
                        //
                        Ok(if computed {
                            let index_type =
                                self.type_of(&prop).map(Cow::into_owned).map(Box::new)?;
                            TsIndexedAccessType {
                                span,
                                obj_type,
                                index_type,
                            }
                        } else {
                            TsIndexedAccessType {
                                span,
                                obj_type,
                                index_type: box TsType::TsKeywordType(TsKeywordType {
                                    span,
                                    kind: TsKeywordTypeKind::TsStringKeyword,
                                }),
                            }
                        })
                    })
                    .map(|res| res.map(TsType::TsIndexedAccessType))
                    .map(|res| res.map(Cow::Owned))??;

                obj_ty
            }

            Expr::MetaProp(..) => unimplemented!("typeof(MetaProp)"),

            Expr::Assign(AssignExpr { ref right, .. }) => return self.type_of(right),

            Expr::Bin(BinExpr {
                op: op!("||"),
                ref right,
                ..
            })
            | Expr::Bin(BinExpr {
                op: op!("&&"),
                ref right,
                ..
            }) => return self.type_of(&right),

            Expr::Bin(BinExpr {
                op: op!(bin, "-"), ..
            })
            | Expr::Bin(BinExpr {
                op: op!(bin, "+"), ..
            }) => Cow::Owned(TsType::TsKeywordType(TsKeywordType {
                kind: TsKeywordTypeKind::TsNumberKeyword,
                span,
            })),

            Expr::Bin(BinExpr { op: op!("==="), .. })
            | Expr::Bin(BinExpr { op: op!("!=="), .. })
            | Expr::Bin(BinExpr { op: op!("!="), .. })
            | Expr::Bin(BinExpr { op: op!("=="), .. })
            | Expr::Bin(BinExpr { op: op!("<="), .. })
            | Expr::Bin(BinExpr { op: op!("<"), .. })
            | Expr::Bin(BinExpr { op: op!(">="), .. })
            | Expr::Bin(BinExpr { op: op!(">"), .. }) => {
                Cow::Owned(TsType::TsKeywordType(TsKeywordType {
                    span,
                    kind: TsKeywordTypeKind::TsBooleanKeyword,
                }))
            }

            Expr::Unary(UnaryExpr {
                op: op!("void"), ..
            }) => Cow::Owned(undefined(span)),

            _ => unimplemented!("typeof ({:#?})", expr),
        })
    }

    fn type_of_prop(&self, prop: &Prop) -> TsTypeElement {
        TsPropertySignature {
            span: prop.span(),
            key: prop_key_to_expr(&prop),
            params: Default::default(),
            init: None,
            optional: false,
            readonly: false,
            computed: false,
            type_ann: Default::default(),
            type_params: Default::default(),
        }
        .into()
    }

    pub(super) fn type_of_class(&self, c: &Class) -> Result<TsType, Error> {
        let mut type_props = vec![];
        for member in &c.body {
            let span = member.span();
            let any = any(span);

            match member {
                ClassMember::ClassProp(ref p) => {
                    let ty = match p.type_ann.as_ref().map(|ty| &*ty.type_ann) {
                        Some(ty) => Cow::Borrowed(ty),
                        None => match p.value {
                            Some(ref e) => self.type_of(&e)?,
                            None => Cow::Owned(any),
                        },
                    };

                    type_props.push(TsTypeElement::TsPropertySignature(TsPropertySignature {
                        span,
                        key: p.key.clone(),
                        optional: p.is_optional,
                        readonly: p.readonly,
                        init: p.value.clone(),
                        type_ann: Some(TsTypeAnn {
                            span: ty.span(),
                            type_ann: box ty.into_owned(),
                        }),

                        // TODO(kdy1):
                        computed: false,

                        // TODO(kdy1):
                        params: Default::default(),

                        // TODO(kdy1):
                        type_params: Default::default(),
                    }));
                }

                // TODO(kdy1):
                ClassMember::Constructor(ref c) => {
                    type_props.push(TsTypeElement::TsConstructSignatureDecl(
                        TsConstructSignatureDecl {
                            span,

                            // TODO(kdy1):
                            type_ann: None,

                            params: c
                                .params
                                .iter()
                                .map(|param| match *param {
                                    PatOrTsParamProp::Pat(ref pat) => {
                                        pat_to_ts_fn_param(pat.clone())
                                    }
                                    PatOrTsParamProp::TsParamProp(ref prop) => match prop.param {
                                        TsParamPropParam::Ident(ref i) => {
                                            TsFnParam::Ident(i.clone())
                                        }
                                        TsParamPropParam::Assign(AssignPat {
                                            ref left, ..
                                        }) => pat_to_ts_fn_param(*left.clone()),
                                    },
                                })
                                .collect(),

                            // TODO(kdy1):
                            type_params: Default::default(),
                        },
                    ));
                }

                // TODO(kdy1):
                ClassMember::Method(..) => {}

                // TODO(kdy1):
                ClassMember::TsIndexSignature(..) => {}

                ClassMember::PrivateMethod(..) | ClassMember::PrivateProp(..) => {}
            }
        }

        Ok(TsType::TsTypeLit(TsTypeLit {
            span: c.span(),
            members: type_props,
        }))
    }

    pub(super) fn infer_return_type(&self, body: &BlockStmt) -> Result<TsType, Error> {
        let mut types = vec![];

        struct Visitor<'a> {
            a: &'a Analyzer<'a, 'a>,
            span: Span,
            types: &'a mut Vec<Result<TsType, Error>>,
        }

        impl Visit<ReturnStmt> for Visitor<'_> {
            fn visit(&mut self, stmt: &ReturnStmt) {
                let ty = match stmt.arg {
                    Some(ref arg) => self.a.type_of(arg),
                    None => Ok(Cow::Owned(undefined(self.span))),
                };
                self.types.push(ty.map(|ty| ty.into_owned()));
            }
        }
        let types_len = types.len();
        let types = {
            let mut v = Visitor {
                span: body.span(),
                types: &mut types,
                a: self,
            };
            body.visit_with(&mut v);
            types
        };

        let mut tys = Vec::with_capacity(types_len);
        for ty in types {
            let ty = ty?;
            tys.push(box ty);
        }

        match tys.len() {
            0 => Ok(undefined(body.span())),
            1 => Ok(*tys.into_iter().next().unwrap()),
            _ => Ok(TsType::TsUnionOrIntersectionType(
                TsUnionOrIntersectionType::TsUnionType(TsUnionType {
                    span: body.span(),
                    types: tys,
                }),
            )),
        }
    }

    pub(super) fn type_of_fn(&self, f: &Function) -> Result<TsType, Error> {
        let ret_ty = match f.return_type {
            Some(ref ret_ty) => ret_ty.clone(),
            None => match f.body {
                Some(ref body) => self.infer_return_type(body).map(|ty| TsTypeAnn {
                    span: ty.span(),
                    type_ann: box ty,
                })?,
                None => unreachable!("function without body should have type annotation"),
            },
        };

        Ok(TsType::TsFnOrConstructorType(
            TsFnOrConstructorType::TsFnType(TsFnType {
                span: f.span,
                params: f.params.iter().cloned().map(pat_to_ts_fn_param).collect(),
                type_params: f.type_params.clone(),
                type_ann: ret_ty,
            }),
        ))
    }

    fn extract_call_new_expr(
        &self,
        callee: &Expr,
        kind: ExtractKind,
        args: &[ExprOrSpread],
        type_args: Option<&TsTypeParamInstantiation>,
    ) -> Result<Cow<TsType>, Error> {
        let span = callee.span();

        match *callee {
            Expr::Ident(ref i) if i.sym == js_word!("require") => {
                if let Some(dep) = self.resolved_imports.get(
                    &args
                        .iter()
                        .cloned()
                        .map(|arg| match arg {
                            ExprOrSpread { spread: None, expr } => match *expr {
                                Expr::Lit(Lit::Str(Str { value, .. })) => value.clone(),
                                _ => unimplemented!("dynamic import: require()"),
                            },
                            _ => unimplemented!("error reporting: spread element in require()"),
                        })
                        .next()
                        .unwrap(),
                ) {
                    let dep = dep.clone();
                    unimplemented!("dep: {:#?}", dep);
                } else {
                    Err(Error::UndefinedSymbol { span: i.span() })
                }
            }

            Expr::Member(MemberExpr {
                obj: ExprOrSuper::Expr(ref obj),
                ref prop,
                computed,
                ..
            }) => {
                // member expression
                let obj_type = self.type_of(obj)?;

                match *obj_type {
                    TsType::TsTypeLit(TsTypeLit { ref members, .. }) => {
                        // Candidates of the method call.
                        //
                        // 4 is just an unsientific guess
                        let mut candidates = Vec::with_capacity(4);

                        for m in members {
                            match m {
                                TsTypeElement::TsMethodSignature(ref m)
                                    if kind == ExtractKind::Call =>
                                {
                                    // We are only interested on methods named `prop`
                                    if prop.eq_ignore_span(&m.key) {
                                        candidates.push(m.clone());
                                    }
                                }

                                _ => {}
                            }
                        }

                        match candidates.len() {
                            0 => {}
                            1 => {
                                let TsMethodSignature { type_ann, .. } =
                                    candidates.into_iter().next().unwrap();

                                return Ok(Cow::Owned(
                                    type_ann.map(|ty| *ty.type_ann).unwrap_or_else(|| any(span)),
                                ));
                            }
                            _ => {
                                //
                                for c in candidates {
                                    if c.params.len() == args.len() {
                                        return Ok(Cow::Owned(
                                            c.type_ann
                                                .map(|ty| *ty.type_ann)
                                                .unwrap_or_else(|| any(span)),
                                        ));
                                    }
                                }

                                unimplemented!(
                                    "multiple methods with same name and same number of arguments"
                                )
                            }
                        }
                    }

                    TsType::TsKeywordType(TsKeywordType {
                        kind: TsKeywordTypeKind::TsAnyKeyword,
                        ..
                    }) => {
                        return Ok(Cow::Owned(any(span)));
                    }

                    _ => {}
                }

                if computed {
                    // let index_type = self.type_of(&prop).map(Cow::into_owned).map(Box::new)?;
                    unimplemented!("typeeof(CallExpr): {:?}[{:?}]()", callee, prop)
                } else {
                    Err(if kind == ExtractKind::Call {
                        Error::NoCallSignature { span }
                    } else {
                        Error::NoNewSignature { span }
                    })
                }
            }
            _ => {
                let ty = self.type_of(callee)?;

                self.extract(ty, kind, args, type_args)
            }
        }
    }

    fn extract(
        &self,
        ty: Cow<TsType>,
        kind: ExtractKind,
        args: &[ExprOrSpread],
        type_args: Option<&TsTypeParamInstantiation>,
    ) -> Result<Cow<TsType>, Error> {
        let span = ty.span();
        let any = any(span);

        macro_rules! ret_err {
            () => {
                match kind {
                    ExtractKind::Call => return Err(Error::NoCallSignature { span: ty.span() }),
                    ExtractKind::New => return Err(Error::NoNewSignature { span: ty.span() }),
                }
            };
        }

        match *ty {
            TsType::TsTypeLit(ref lit) => {
                for member in &lit.members {
                    match *member {
                        TsTypeElement::TsCallSignatureDecl(TsCallSignatureDecl {
                            ref params,
                            ref type_params,
                            ref type_ann,
                            ..
                        }) if kind == ExtractKind::Call => {
                            //
                            match self
                                .try_instantiate(
                                    span,
                                    &type_ann
                                        .as_ref()
                                        .map(|v| &*v.type_ann)
                                        .unwrap_or_else(|| &any),
                                    params,
                                    type_params.as_ref(),
                                    args,
                                    type_args,
                                )
                                .map(Cow::Owned)
                            {
                                Ok(v) => return Ok(v),
                                Err(..) => {}
                            };
                        }

                        TsTypeElement::TsConstructSignatureDecl(TsConstructSignatureDecl {
                            ref params,
                            ref type_params,
                            ref type_ann,
                            ..
                        }) if kind == ExtractKind::New => {
                            match self
                                .try_instantiate(
                                    span,
                                    &type_ann
                                        .as_ref()
                                        .map(|v| &*v.type_ann)
                                        .unwrap_or_else(|| &any),
                                    params,
                                    type_params.as_ref(),
                                    args,
                                    type_args,
                                )
                                .map(Cow::Owned)
                            {
                                Ok(v) => return Ok(v),
                                Err(..) => {
                                    // TODO: Handle error
                                }
                            }
                        }
                        _ => {}
                    }
                }

                ret_err!()
            }

            TsType::TsFnOrConstructorType(ref f_c) => match *f_c {
                TsFnOrConstructorType::TsFnType(TsFnType {
                    ref params,
                    ref type_params,
                    ref type_ann,
                    ..
                }) if kind == ExtractKind::Call => self
                    .try_instantiate(
                        span,
                        &type_ann.type_ann,
                        params,
                        type_params.as_ref(),
                        args,
                        type_args,
                    )
                    .map(Cow::Owned),
                TsFnOrConstructorType::TsConstructorType(TsConstructorType {
                    ref params,
                    ref type_params,
                    ref type_ann,
                    ..
                }) if kind == ExtractKind::New => self
                    .try_instantiate(
                        span,
                        &type_ann.type_ann,
                        params,
                        type_params.as_ref(),
                        args,
                        type_args,
                    )
                    .map(Cow::Owned),

                _ => ret_err!(),
            },

            TsType::TsUnionOrIntersectionType(TsUnionOrIntersectionType::TsUnionType(ref u)) => {
                let mut errors = vec![];
                for ty in &u.types {
                    match self.extract(Cow::Borrowed(&*ty), kind, args, type_args) {
                        Ok(ty) => return Ok(ty),
                        Err(err) => errors.push(err),
                    }
                }

                Err(Error::UnionError { span, errors })
            }

            _ => ret_err!(),
        }
    }

    fn try_instantiate(
        &self,
        span: Span,
        ret_type: &TsType,
        param_decls: &[TsFnParam],
        ty_params_decl: Option<&TsTypeParamDecl>,
        args: &[ExprOrSpread],
        i: Option<&TsTypeParamInstantiation>,
    ) -> Result<TsType, Error> {
        let type_params_len = ty_params_decl.map(|decl| decl.params.len()).unwrap_or(0);
        let type_args_len = i.map(|v| v.params.len()).unwrap_or(0);

        if type_args_len > type_params_len {
            return Err(Error::WrongTypeParams {
                span,
                // TODO
                expected: 0..type_params_len,
                actual: type_args_len,
            });
        }

        if param_decls.len() > args.len() {
            return Err(Error::WrongParams {
                span,
                // TODO
                expected: 0..param_decls.len(),
                actual: type_args_len,
            });
        }

        Ok(ret_type.clone())
    }

    pub(super) fn expand<'t>(&mut self, ty: Cow<'t, TsType>) -> Result<Cow<'t, TsType>, Error> {
        match *ty {
            TsType::TsTypeRef(TsTypeRef {
                ref type_name,
                ref type_params,
                ..
            }) => {
                return self
                    .expand_export_info(type_name, type_params.as_ref())
                    .map(Cow::Owned)
            }
            _ => {}
        }

        Ok(ty)
    }

    fn expand_export_info(
        &self,
        name: &TsEntityName,
        type_params: Option<&TsTypeParamInstantiation>,
    ) -> Result<TsType, Error> {
        let e = (|| {
            fn root(n: &TsEntityName) -> &Ident {
                match *n {
                    TsEntityName::TsQualifiedName(box TsQualifiedName { ref left, .. }) => {
                        root(left)
                    }
                    TsEntityName::Ident(ref i) => i,
                }
            }

            // Search imports / decls.
            let root = root(name);

            if let Some(v) = self.resolved_imports.get(&root.sym) {
                return Ok(&**v);
            }

            if let Some(v) = self.scope.find_type(&root.sym) {
                return Ok(v);
            }

            Err(Error::UndefinedSymbol { span: root.span })
        })()?;

        match e.ty {
            Some(ref ty) => {
                assert_eq!(type_params, None); // TODO: Error
                return Ok(ty.clone());
            }
            None => {}
        }

        match e.extra {
            Some(ref extra) => {
                // Expand
                match extra {
                    ExportExtra::Enum(..) => {
                        assert_eq!(type_params, None);

                        unimplemented!("ExportExtra::Enum -> instantiate()")
                    }
                    ExportExtra::Module(TsModuleDecl {
                        body: Some(body), ..
                    })
                    | ExportExtra::Namespace(TsNamespaceDecl { box body, .. }) => {
                        assert_eq!(type_params, None);

                        unimplemented!("ExportExtra::Namespace -> instantiate()")
                    }
                    ExportExtra::Module(..) => {
                        assert_eq!(type_params, None);

                        unimplemented!("ExportExtra::Module without body cannot be instantiated")
                    }
                    ExportExtra::Interface(ref i) => {
                        // TODO: Check length of type parmaters
                        // TODO: Instantiate type parameters

                        let members = i.body.body.iter().cloned().collect();

                        return Ok(TsType::TsTypeLit(TsTypeLit {
                            span: i.span,
                            members,
                        }));
                    }
                    ExportExtra::Alias(ref decl) => {
                        // TODO(kdy1): Handle type parameters.
                        return Ok(*decl.type_ann.clone());
                    }
                }
            }
            None => unimplemented!("`ty` and `extra` are both null"),
        }
    }
}

fn prop_key_to_expr(p: &Prop) -> Box<Expr> {
    match *p {
        Prop::Shorthand(ref i) => box Expr::Ident(i.clone()),
        Prop::Assign(AssignProp { ref key, .. }) => box Expr::Ident(key.clone()),
        Prop::Getter(GetterProp { ref key, .. })
        | Prop::KeyValue(KeyValueProp { ref key, .. })
        | Prop::Method(MethodProp { ref key, .. })
        | Prop::Setter(SetterProp { ref key, .. }) => match *key {
            PropName::Computed(ref expr) => expr.clone(),
            PropName::Ident(ref ident) => box Expr::Ident(ident.clone()),
            PropName::Str(ref s) => box Expr::Lit(Lit::Str(Str { ..s.clone() })),
            PropName::Num(ref s) => box Expr::Lit(Lit::Num(Number { ..s.clone() })),
        },
    }
}

trait RemoveTypes {
    /// Removes falsy values from `self`.
    fn remove_falsy(self) -> TsType;
}

fn never_ty(span: Span) -> TsType {
    TsType::TsKeywordType(TsKeywordType {
        span,
        kind: TsKeywordTypeKind::TsNeverKeyword,
    })
}

impl RemoveTypes for TsType {
    fn remove_falsy(self) -> TsType {
        match self {
            TsType::TsUnionOrIntersectionType(n) => n.remove_falsy().into(),
            TsType::TsKeywordType(TsKeywordType { kind, span }) => match kind {
                TsKeywordTypeKind::TsUndefinedKeyword | TsKeywordTypeKind::TsNullKeyword => {
                    never_ty(span)
                }
                _ => self,
            },
            _ => self,
        }
    }
}

impl RemoveTypes for TsUnionOrIntersectionType {
    fn remove_falsy(self) -> TsType {
        match self {
            TsUnionOrIntersectionType::TsIntersectionType(n) => n.remove_falsy().into(),
            TsUnionOrIntersectionType::TsUnionType(n) => n.remove_falsy().into(),
        }
    }
}

impl RemoveTypes for TsIntersectionType {
    fn remove_falsy(self) -> TsType {
        let types = self
            .types
            .into_iter()
            .map(|ty| ty.remove_falsy())
            .map(Box::new)
            .collect::<Vec<_>>();
        if types.iter().any(|ty| is_never(&ty)) {
            return TsType::TsKeywordType(TsKeywordType {
                span: self.span,
                kind: TsKeywordTypeKind::TsNeverKeyword,
            });
        }

        TsType::TsUnionOrIntersectionType(TsIntersectionType { types, ..self }.into())
    }
}

impl RemoveTypes for TsUnionType {
    fn remove_falsy(self) -> TsType {
        let types = self
            .types
            .into_iter()
            .map(|ty| ty.remove_falsy())
            .filter(|ty| !is_never(&ty))
            .map(Box::new)
            .collect();

        TsType::TsUnionOrIntersectionType(TsUnionType { types, ..self }.into())
    }
}

impl RemoveTypes for Box<TsType> {
    fn remove_falsy(self) -> TsType {
        (*self).remove_falsy()
    }
}

fn is_never(ty: &TsType) -> bool {
    match *ty {
        TsType::TsKeywordType(TsKeywordType {
            kind: TsKeywordTypeKind::TsNeverKeyword,
            ..
        }) => false,
        _ => true,
    }
}

fn negate(ty: Cow<TsType>) -> Cow<TsType> {
    fn boolean(span: Span) -> TsType {
        TsType::TsKeywordType(TsKeywordType {
            span,
            kind: TsKeywordTypeKind::TsBooleanKeyword,
        })
    }

    Cow::Owned(match *ty {
        TsType::TsLitType(TsLitType { ref lit, span }) => match *lit {
            TsLit::Bool(v) => TsType::TsLitType(TsLitType {
                lit: TsLit::Bool(Bool {
                    value: !v.value,
                    ..v
                }),
                span,
            }),
            TsLit::Number(v) => TsType::TsLitType(TsLitType {
                lit: TsLit::Bool(Bool {
                    value: v.value != 0.0,
                    span: v.span,
                }),
                span,
            }),
            TsLit::Str(ref v) => TsType::TsLitType(TsLitType {
                lit: TsLit::Bool(Bool {
                    value: v.value != js_word!(""),
                    span: v.span,
                }),
                span,
            }),
        },
        _ => boolean(ty.span()),
    })
}

pub const fn undefined(span: Span) -> TsType {
    TsType::TsKeywordType(TsKeywordType {
        span,
        kind: TsKeywordTypeKind::TsUndefinedKeyword,
    })
}

pub const fn any(span: Span) -> TsType {
    TsType::TsKeywordType(TsKeywordType {
        span,
        kind: TsKeywordTypeKind::TsAnyKeyword,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExtractKind {
    Call,
    New,
}
