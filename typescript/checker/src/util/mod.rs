use crate::ty::{Class, Intersection, Type, TypeElement, Union};
use std::{borrow::Cow, mem::transmute};
use swc_atoms::js_word;
use swc_common::{Fold, FoldWith, Span, DUMMY_SP};
use swc_ecma_ast::*;

pub(crate) mod named;

pub trait ModuleItemLike: StmtLike {
    fn try_into_module_decl(self) -> Result<ModuleDecl, Self> {
        Err(self)
    }
    fn try_from_module_decl(decl: ModuleDecl) -> Result<Self, ModuleDecl> {
        Err(decl)
    }
}

pub trait StmtLike: Sized {
    fn try_into_stmt(self) -> Result<Stmt, Self>;
    fn as_stmt(&self) -> Option<&Stmt>;
    fn from_stmt(stmt: Stmt) -> Self;
}

impl ModuleItemLike for Stmt {}

impl StmtLike for Stmt {
    fn try_into_stmt(self) -> Result<Stmt, Self> {
        Ok(self)
    }
    fn as_stmt(&self) -> Option<&Stmt> {
        Some(&self)
    }
    fn from_stmt(stmt: Stmt) -> Self {
        stmt
    }
}

impl ModuleItemLike for ModuleItem {
    fn try_into_module_decl(self) -> Result<ModuleDecl, Self> {
        match self {
            ModuleItem::ModuleDecl(decl) => Ok(decl),
            _ => Err(self),
        }
    }
    fn try_from_module_decl(decl: ModuleDecl) -> Result<Self, ModuleDecl> {
        Ok(ModuleItem::ModuleDecl(decl))
    }
}
impl StmtLike for ModuleItem {
    fn try_into_stmt(self) -> Result<Stmt, Self> {
        match self {
            ModuleItem::Stmt(stmt) => Ok(stmt),
            _ => Err(self),
        }
    }
    fn as_stmt(&self) -> Option<&Stmt> {
        match *self {
            ModuleItem::Stmt(ref stmt) => Some(stmt),
            _ => None,
        }
    }
    fn from_stmt(stmt: Stmt) -> Self {
        ModuleItem::Stmt(stmt)
    }
}

pub trait EqIgnoreSpan {
    fn eq_ignore_span(&self, to: &Self) -> bool;
}

pub trait EqIgnoreNameAndSpan<T = Self> {
    fn eq_ignore_name_and_span(&self, to: &T) -> bool;
}

macro_rules! impl_by_clone {
    ($T:ty) => {
        impl EqIgnoreSpan for $T {
            fn eq_ignore_span(&self, to: &Self) -> bool {
                self.clone().fold_with(&mut SpanRemover) == to.clone().fold_with(&mut SpanRemover)
            }
        }

        impl EqIgnoreNameAndSpan<$T> for $T {
            fn eq_ignore_name_and_span(&self, to: &$T) -> bool {
                let l = self.clone().fold_with(&mut SpanAndNameRemover);
                let r = to.clone().fold_with(&mut SpanAndNameRemover);
                // In current implementation, l and r lives until the functions return, so it's
                // safe.

                // TODO: Ensure that this is correct.
                let r: Self = unsafe { transmute(r) };
                l == r
            }
        }
    };
}
impl_by_clone!(Type);
impl_by_clone!(Expr);
impl_by_clone!(TypeElement);
impl_by_clone!(TsLit);
impl_by_clone!(TsLitType);
impl_by_clone!(PropName);
impl_by_clone!(Class);

struct SpanRemover;
impl Fold<Span> for SpanRemover {
    fn fold(&mut self, _: Span) -> Span {
        DUMMY_SP
    }
}

struct SpanAndNameRemover;
impl Fold<Span> for SpanAndNameRemover {
    fn fold(&mut self, _: Span) -> Span {
        DUMMY_SP
    }
}
impl Fold<Ident> for SpanAndNameRemover {
    fn fold(&mut self, i: Ident) -> Ident {
        Ident {
            sym: js_word!(""),
            span: DUMMY_SP,
            ..i.fold_children(self)
        }
    }
}

/// TsEntityName is used in type position.
impl Fold<TsEntityName> for SpanAndNameRemover {
    fn fold(&mut self, n: TsEntityName) -> TsEntityName {
        match n {
            TsEntityName::Ident(i) => TsEntityName::Ident(Ident {
                span: DUMMY_SP,
                ..i.fold_children(self)
            }),
            TsEntityName::TsQualifiedName(q) => TsEntityName::TsQualifiedName(q.fold_with(self)),
        }
    }
}

impl Fold<TsQualifiedName> for SpanRemover {
    fn fold(&mut self, n: TsQualifiedName) -> TsQualifiedName {
        TsQualifiedName {
            left: n.left.fold_with(self),
            right: Ident {
                span: DUMMY_SP,
                ..n.right
            },
        }
    }
}

impl<T> EqIgnoreSpan for Box<T>
where
    T: EqIgnoreSpan,
{
    fn eq_ignore_span(&self, to: &Self) -> bool {
        (**self).eq_ignore_span(&**to)
    }
}

impl<T> EqIgnoreSpan for Option<T>
where
    T: EqIgnoreSpan,
{
    fn eq_ignore_span(&self, to: &Self) -> bool {
        match (self.as_ref(), to.as_ref()) {
            (Some(l), Some(r)) => l.eq_ignore_span(r),
            (None, None) => true,
            _ => false,
        }
    }
}

impl<T> EqIgnoreSpan for Vec<T>
where
    T: EqIgnoreSpan,
{
    fn eq_ignore_span(&self, to: &Self) -> bool {
        if self.len() != to.len() {
            return false;
        }

        self.iter()
            .zip(to.iter())
            .all(|(l, r)| l.eq_ignore_span(&r))
    }
}

impl<T> EqIgnoreNameAndSpan for Box<T>
where
    T: EqIgnoreNameAndSpan,
{
    fn eq_ignore_name_and_span(&self, to: &Self) -> bool {
        (**self).eq_ignore_name_and_span(&**to)
    }
}

impl<T> EqIgnoreNameAndSpan for Option<T>
where
    T: EqIgnoreNameAndSpan,
{
    fn eq_ignore_name_and_span(&self, to: &Self) -> bool {
        match (self.as_ref(), to.as_ref()) {
            (Some(l), Some(r)) => l.eq_ignore_name_and_span(r),
            (None, None) => true,
            _ => false,
        }
    }
}

impl<T> EqIgnoreNameAndSpan for Vec<T>
where
    T: EqIgnoreNameAndSpan,
{
    fn eq_ignore_name_and_span(&self, to: &Self) -> bool {
        if self.len() != to.len() {
            return false;
        }

        self.iter()
            .zip(to.iter())
            .all(|(l, r)| l.eq_ignore_name_and_span(&r))
    }
}

pub trait IntoCow<'a, T>: Sized + Into<T>
where
    T: Clone,
{
    fn into_cow(self) -> Cow<'a, T> {
        Cow::Owned(self.into())
    }
}

impl<T, S> IntoCow<'_, T> for S
where
    Self: Into<T>,
    T: Clone,
{
}

pub(crate) fn pat_to_ts_fn_param(p: Pat) -> TsFnParam {
    match p {
        Pat::Ident(i) => TsFnParam::Ident(i),
        Pat::Rest(rest) => TsFnParam::Rest(rest),
        Pat::Array(arr) => TsFnParam::Array(arr),
        Pat::Object(obj) => TsFnParam::Object(obj),
        // TODO: Pat::Assign()
        Pat::Assign(assign) => pat_to_ts_fn_param(*assign.left),
        _ => unreachable!("pat_to_ts_fn_param: Pat: {:?}", p),
    }
}

pub(crate) trait RemoveTypes {
    /// Removes falsy values from `self`.
    fn remove_falsy(self) -> Type;

    /// Removes truthy values from `self`.
    fn remove_truthy(self) -> Type;
}

impl RemoveTypes for Type {
    fn remove_falsy(self) -> Type {
        match self {
            Type::Keyword(TsKeywordType { kind, span }) => match kind {
                TsKeywordTypeKind::TsUndefinedKeyword | TsKeywordTypeKind::TsNullKeyword => {
                    return Type::never(span);
pub(crate) trait RemoveTypes<'a> {
    /// Removes falsy values from `self`.
    fn remove_falsy(self) -> Type;

    /// Removes truthy values from `self`.
    fn remove_truthy(self) -> Type;
}

impl RemoveTypes for Type {
    fn remove_falsy(self) -> Type {
        match self {
            Type::Keyword(TsKeywordType { kind, span }) => match kind {
                TsKeywordTypeKind::TsUndefinedKeyword | TsKeywordTypeKind::TsNullKeyword => {
                    return Type::never(span).into_cow();
                }
                _ => {}
            },
            Type::Lit(TsLitType {
                lit:
                    TsLit::Bool(Bool {
                        value: false, span, ..
                    }),
                ..
            }) => return Type::never(span),
            }) => return Type::never(span).into_cow(),

            Type::Union(u) => return u.remove_falsy(),
            Type::Intersection(i) => return i.remove_falsy(),
            _ => {}
        }

        Cow::Owned(self)
    }

    fn remove_truthy(self) -> Type {
    fn remove_truthy(self) -> TypeRef<'a> {
        match self {
            Type::Lit(TsLitType {
                lit: TsLit::Bool(Bool {
                    value: true, span, ..
                }),
                ..
            }) => return Type::never(span),
            }) => return Type::never(span).into_cow(),

            Type::Union(u) => u.remove_truthy(),
            Type::Intersection(i) => i.remove_truthy(),
            _ => Cow::Owned(self),
        }
    }
}

impl RemoveTypes for Intersection {
    fn remove_falsy(self) -> Type {
impl<'a> RemoveTypes<'a> for Intersection<'a> {
    fn remove_falsy(self) -> Type {
        let types = self
            .types
            .into_iter()
            .map(|ty| ty.remove_falsy())
            .collect::<Vec<_>>();
        if types.iter().any(|ty| ty.is_never()) {
            return Type::never(self.span);
            return Type::never(self.span).into_cow();
        }

        Intersection {
            span: self.span,
            types,
        }
        .into()
    }

    fn remove_truthy(self) -> Type {
        .into_cow()
    }

    fn remove_truthy(self) -> Type {
        let types = self
            .types
            .into_iter()
            .map(|ty| ty.remove_truthy())
            .collect::<Vec<_>>();
        if types.iter().any(|ty| ty.is_never()) {
            return Type::never(self.span);
            return Type::never(self.span).into_cow();
        }

        Intersection {
            span: self.span,
            types,
        }
        .into()
    }
}

impl RemoveTypes for Union {
    fn remove_falsy(self) -> Type {
        .into_cow()
    }
}

impl RemoveTypes for Union {
    fn remove_falsy(self) -> Type {
        let types = self
            .types
            .into_iter()
            .map(|ty| ty.remove_falsy())
            .filter(|ty| !ty.is_never())
            .collect();
        Union {
            span: self.span,
            types,
        }
        .into()
    }

    fn remove_truthy(self) -> Type {
        .into_cow()
    }

    fn remove_truthy(self) -> Type {
        let types = self
            .types
            .into_iter()
            .map(|ty| ty.remove_truthy())
            .filter(|ty| !ty.is_never())
            .collect();
        Union {
            span: self.span,
            types,
        }
        .into()
    }
}

impl<'a, T> RemoveTypes for Box<T>
where
    T: RemoveTypes,
{
    fn remove_falsy(self) -> Type {
        (*self).remove_falsy()
    }

    fn remove_truthy(self) -> Type {
        .into_cow()
    }
}

impl<'a, T> RemoveTypes for Box<T>
where
    T: RemoveTypes,
{
    fn remove_falsy(self) -> Type {
        (*self).remove_falsy()
    }

    fn remove_truthy(self) -> Type {
        (*self).remove_truthy()
    }
}

pub(crate) trait EndsWithRet {
trait EndsWithRet {
    /// Returns true if the statement ends with return, break, continue;
    fn ends_with_ret(&self) -> bool;
}

impl EndsWithRet for Stmt {
    /// Returns true if the statement ends with return, break, continue;
    fn ends_with_ret(&self) -> bool {
        match *self {
            Stmt::Return(..) | Stmt::Break(..) | Stmt::Continue(..) | Stmt::Throw(..) => true,
            Stmt::Block(ref stmt) => stmt.ends_with_ret(),
            _ => false,
        }
    }
}

impl EndsWithRet for BlockStmt {
    /// Returns true if the statement ends with return, break, continue;
    fn ends_with_ret(&self) -> bool {
        self.stmts.ends_with_ret()
    }
}

impl<T> EndsWithRet for Vec<T>
where
    T: EndsWithRet,
{
    /// Returns true if the statement ends with return, break, continue;
    fn ends_with_ret(&self) -> bool {
        match self.last() {
            Some(ref stmt) => stmt.ends_with_ret(),
            _ => false,
        }
    }
}
