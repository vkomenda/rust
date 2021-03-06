// Copyright 2016 The Rust Project Developers. See the COPYRIGHT
// file at the top-level directory of this distribution and at
// http://rust-lang.org/COPYRIGHT.
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

//! Walks the crate looking for items/impl-items/trait-items that have
//! either a `rustc_symbol_name` or `rustc_item_path` attribute and
//! generates an error giving, respectively, the symbol name or
//! item-path. This is used for unit testing the code that generates
//! paths etc in all kinds of annoying scenarios.

use asm;
use attributes;
use base;
use consts;
use context::CodegenCx;
use declare;
use llvm;
use monomorphize::Instance;
use type_of::LayoutLlvmExt;
use rustc::hir;
use rustc::mir::mono::{Linkage, Visibility};
use rustc::ty::{TyCtxt, TypeFoldable};
use rustc::ty::layout::LayoutOf;
use syntax::ast;
use syntax::attr;
use syntax_pos::Span;
use std::fmt;

pub use rustc::mir::mono::MonoItem;

pub use rustc_mir::monomorphize::item::*;
pub use rustc_mir::monomorphize::item::MonoItemExt as BaseMonoItemExt;

pub trait MonoItemExt<'a, 'tcx>: fmt::Debug + BaseMonoItemExt<'a, 'tcx> {
    fn define(&self, cx: &CodegenCx<'a, 'tcx>) {
        debug!("BEGIN IMPLEMENTING '{} ({})' in cgu {}",
               self.to_string(cx.tcx),
               self.to_raw_string(),
               cx.codegen_unit.name());

        match *self.as_mono_item() {
            MonoItem::Static(node_id) => {
                let tcx = cx.tcx;
                let item = tcx.hir.expect_item(node_id);
                if let hir::ItemStatic(_, m, _) = item.node {
                    match consts::trans_static(&cx, m, item.id, &item.attrs) {
                        Ok(_) => { /* Cool, everything's alright. */ },
                        Err(err) => {
                            err.report(tcx, item.span, "static");
                        }
                    };
                } else {
                    span_bug!(item.span, "Mismatch between hir::Item type and TransItem type")
                }
            }
            MonoItem::GlobalAsm(node_id) => {
                let item = cx.tcx.hir.expect_item(node_id);
                if let hir::ItemGlobalAsm(ref ga) = item.node {
                    asm::trans_global_asm(cx, ga);
                } else {
                    span_bug!(item.span, "Mismatch between hir::Item type and TransItem type")
                }
            }
            MonoItem::Fn(instance) => {
                base::trans_instance(&cx, instance);
            }
        }

        debug!("END IMPLEMENTING '{} ({})' in cgu {}",
               self.to_string(cx.tcx),
               self.to_raw_string(),
               cx.codegen_unit.name());
    }

    fn predefine(&self,
                 cx: &CodegenCx<'a, 'tcx>,
                 linkage: Linkage,
                 visibility: Visibility) {
        debug!("BEGIN PREDEFINING '{} ({})' in cgu {}",
               self.to_string(cx.tcx),
               self.to_raw_string(),
               cx.codegen_unit.name());

        let symbol_name = self.symbol_name(cx.tcx);

        debug!("symbol {}", &symbol_name);

        match *self.as_mono_item() {
            MonoItem::Static(node_id) => {
                predefine_static(cx, node_id, linkage, visibility, &symbol_name);
            }
            MonoItem::Fn(instance) => {
                predefine_fn(cx, instance, linkage, visibility, &symbol_name);
            }
            MonoItem::GlobalAsm(..) => {}
        }

        debug!("END PREDEFINING '{} ({})' in cgu {}",
               self.to_string(cx.tcx),
               self.to_raw_string(),
               cx.codegen_unit.name());
    }

    fn local_span(&self, tcx: TyCtxt<'a, 'tcx, 'tcx>) -> Option<Span> {
        match *self.as_mono_item() {
            MonoItem::Fn(Instance { def, .. }) => {
                tcx.hir.as_local_node_id(def.def_id())
            }
            MonoItem::Static(node_id) |
            MonoItem::GlobalAsm(node_id) => {
                Some(node_id)
            }
        }.map(|node_id| tcx.hir.span(node_id))
    }

    fn to_raw_string(&self) -> String {
        match *self.as_mono_item() {
            MonoItem::Fn(instance) => {
                format!("Fn({:?}, {})",
                         instance.def,
                         instance.substs.as_ptr() as usize)
            }
            MonoItem::Static(id) => {
                format!("Static({:?})", id)
            }
            MonoItem::GlobalAsm(id) => {
                format!("GlobalAsm({:?})", id)
            }
        }
    }
}

impl<'a, 'tcx> MonoItemExt<'a, 'tcx> for MonoItem<'tcx> {}

fn predefine_static<'a, 'tcx>(cx: &CodegenCx<'a, 'tcx>,
                              node_id: ast::NodeId,
                              linkage: Linkage,
                              visibility: Visibility,
                              symbol_name: &str) {
    let def_id = cx.tcx.hir.local_def_id(node_id);
    let instance = Instance::mono(cx.tcx, def_id);
    let ty = instance.ty(cx.tcx);
    let llty = cx.layout_of(ty).llvm_type(cx);

    let g = declare::define_global(cx, symbol_name, llty).unwrap_or_else(|| {
        cx.sess().span_fatal(cx.tcx.hir.span(node_id),
            &format!("symbol `{}` is already defined", symbol_name))
    });

    unsafe {
        llvm::LLVMRustSetLinkage(g, base::linkage_to_llvm(linkage));
        llvm::LLVMRustSetVisibility(g, base::visibility_to_llvm(visibility));
    }

    cx.instances.borrow_mut().insert(instance, g);
    cx.statics.borrow_mut().insert(g, def_id);
}

fn predefine_fn<'a, 'tcx>(cx: &CodegenCx<'a, 'tcx>,
                          instance: Instance<'tcx>,
                          linkage: Linkage,
                          visibility: Visibility,
                          symbol_name: &str) {
    assert!(!instance.substs.needs_infer() &&
            !instance.substs.has_param_types());

    let mono_ty = instance.ty(cx.tcx);
    let attrs = instance.def.attrs(cx.tcx);
    let lldecl = declare::declare_fn(cx, symbol_name, mono_ty);
    unsafe { llvm::LLVMRustSetLinkage(lldecl, base::linkage_to_llvm(linkage)) };
    base::set_link_section(cx, lldecl, &attrs);
    if linkage == Linkage::LinkOnceODR ||
        linkage == Linkage::WeakODR {
        llvm::SetUniqueComdat(cx.llmod, lldecl);
    }

    // If we're compiling the compiler-builtins crate, e.g. the equivalent of
    // compiler-rt, then we want to implicitly compile everything with hidden
    // visibility as we're going to link this object all over the place but
    // don't want the symbols to get exported.
    if linkage != Linkage::Internal && linkage != Linkage::Private &&
       attr::contains_name(cx.tcx.hir.krate_attrs(), "compiler_builtins") {
        unsafe {
            llvm::LLVMRustSetVisibility(lldecl, llvm::Visibility::Hidden);
        }
    } else {
        unsafe {
            llvm::LLVMRustSetVisibility(lldecl, base::visibility_to_llvm(visibility));
        }
    }

    debug!("predefine_fn: mono_ty = {:?} instance = {:?}", mono_ty, instance);
    if instance.def.is_inline(cx.tcx) {
        attributes::inline(lldecl, attributes::InlineAttr::Hint);
    }
    attributes::from_fn_attrs(cx, lldecl, instance.def.def_id());

    cx.instances.borrow_mut().insert(instance, lldecl);
}
