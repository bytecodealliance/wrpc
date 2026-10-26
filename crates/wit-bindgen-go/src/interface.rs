use core::iter;

use std::fmt::Write as _;
use std::mem;

use heck::ToUpperCamelCase;
use wit_bindgen_core::wit_parser::{
    Case, Docs, Enum, EnumCase, Field, Flag, Flags, Function, FunctionKind, Handle, Int,
    InterfaceId, Param, Record, Resolve, Result_, Tuple, Type, TypeDefKind, TypeId, TypeOwner,
    Variant, WorldKey,
};
use wit_bindgen_core::{Source, TypeInfo, uwrite, uwriteln};
use wrpc_introspect::{async_paths_ty, is_tuple, is_ty, rpc_func_name};

use crate::{
    Deps, GoWrpc, Identifier, InterfaceName, to_go_ident, to_package_ident, to_upper_camel_case,
};

fn go_func_name(func: &Function) -> String {
    match &func.kind {
        FunctionKind::Constructor(..) => to_upper_camel_case(
            func.name
                .strip_prefix("[constructor]")
                .expect("failed to strip `[constructor]` prefix"),
        ),
        FunctionKind::Static(..) => {
            let name = func
                .name
                .strip_prefix("[static]")
                .expect("failed to strip `[static]` prefix");
            let (head, tail) = name.split_once('.').expect("failed to split on `.`");
            format!(
                "{}_{}",
                head.to_upper_camel_case(),
                tail.to_upper_camel_case()
            )
        }
        FunctionKind::Method(..) => {
            let name = func
                .name
                .strip_prefix("[method]")
                .expect("failed to strip `[method]` prefix");
            let (head, tail) = name.split_once('.').expect("failed to split on `.`");
            format!(
                "{}_{}",
                head.to_upper_camel_case(),
                tail.to_upper_camel_case()
            )
        }
        FunctionKind::AsyncStatic(..) => {
            let name = func
                .name
                .strip_prefix("[static]")
                .expect("failed to strip `[static]` prefix");
            let (head, tail) = name.split_once('.').expect("failed to split on `.`");
            format!(
                "{}_{}",
                head.to_upper_camel_case(),
                tail.to_upper_camel_case()
            )
        }
        FunctionKind::AsyncMethod(..) => {
            let name = func
                .name
                .strip_prefix("[method]")
                .expect("failed to strip `[method]` prefix");
            let (head, tail) = name.split_once('.').expect("failed to split on `.`");
            format!(
                "{}_{}",
                head.to_upper_camel_case(),
                tail.to_upper_camel_case()
            )
        }
        FunctionKind::AsyncFreestanding => to_upper_camel_case(&func.name),
        FunctionKind::Freestanding => to_upper_camel_case(&func.name),
    }
}

pub fn flatten_ty<'a>(
    resolve: &'a Resolve,
    ty: &Type,
) -> impl Iterator<Item = Type> + 'a + use<'a> {
    let mut ty = *ty;
    loop {
        if let Type::Id(id) = ty {
            match resolve.types[id].kind {
                TypeDefKind::Type(t) => {
                    ty = t;
                    continue;
                }
                TypeDefKind::Tuple(ref t) => {
                    return Box::new(t.types.iter().copied()) as Box<dyn Iterator<Item = _>>;
                }
                _ => {}
            }
        }
        return Box::new(iter::once(ty)) as Box<dyn Iterator<Item = _>>;
    }
}

pub struct InterfaceGenerator<'a> {
    pub src: Source,
    pub(super) identifier: Identifier<'a>,
    pub in_import: bool,
    pub(super) r#gen: &'a mut GoWrpc,
    pub resolve: &'a Resolve,
    pub deps: Deps,
}

impl InterfaceGenerator<'_> {
    // s{16,32,64} decoding adapted from
    // https://github.com/go-delve/delve/blob/26799555e5518e8a9fe2d68e02379257ebda4dd2/pkg/dwarf/leb128/decode.go#L51-L81
    //
    // s{16,32,64} encoding adapted from
    // https://github.com/go-delve/delve/blob/26799555e5518e8a9fe2d68e02379257ebda4dd2/pkg/dwarf/leb128/encode.go#L23-L42

    fn print_read_bool(&mut self, reader: &str) {
        let fmt = self.deps.fmt();
        let io = self.deps.io();
        let slog = self.deps.slog();
        uwrite!(
            self.src,
            r#"func(r {io}.ByteReader) (bool, error) {{
    {slog}.Debug("reading bool byte")
    v, err := r.ReadByte()
    if err != nil {{
        {slog}.Debug("reading bool", "value", false)
        return false, {fmt}.Errorf("failed to read bool byte: %w", err)
    }}
    switch v {{
        case 0:
            return false, nil
        case 1:
            return true, nil
        default:
            return false, {fmt}.Errorf("invalid bool value %d", v)
    }}
}}({reader})"#,
        );
    }

    fn print_read_u8(&mut self, reader: &str) {
        let fmt = self.deps.fmt();
        let io = self.deps.io();
        let slog = self.deps.slog();
        uwrite!(
            self.src,
            r#"func(r {io}.ByteReader) (uint8, error) {{
    {slog}.Debug("reading u8 byte")
    v, err := r.ReadByte()
    if err != nil {{
        return 0, {fmt}.Errorf("failed to read u8 byte: %w", err)
    }}
    return v, nil
}}({reader})"#,
        );
    }

    fn print_read_u16(&mut self, reader: &str) {
        let fmt = self.deps.fmt();
        let errors = self.deps.errors();
        let io = self.deps.io();
        let slog = self.deps.slog();
        uwrite!(
            self.src,
            r#"func(r {io}.ByteReader) (uint16, error) {{
    var x uint16
    var s uint8
    for i := 0; i < 3; i++ {{
        {slog}.Debug("reading u16 byte", "i", i)
        b, err := r.ReadByte()
        if err != nil {{
            if i > 0 && err == {io}.EOF {{
                err = {io}.ErrUnexpectedEOF
            }}
            return x, {fmt}.Errorf("failed to read u16 byte: %w", err)
        }}
        if s == 14 && b > 0x03 {{
            return x, {errors}.New("varint overflows a 16-bit integer")
        }}
        if b < 0x80 {{
            return x | uint16(b)<<s, nil
        }}
        x |= uint16(b&0x7f) << s
        s += 7
    }}
    return x, {errors}.New("varint overflows a 16-bit integer")
}}({reader})"#,
        );
    }

    fn print_read_u32(&mut self, reader: &str) {
        let fmt = self.deps.fmt();
        let errors = self.deps.errors();
        let io = self.deps.io();
        let slog = self.deps.slog();
        uwrite!(
            self.src,
            r#"func(r {io}.ByteReader) (uint32, error) {{
    var x uint32
    var s uint8
    for i := 0; i < 5; i++ {{
        {slog}.Debug("reading u32 byte", "i", i)
        b, err := r.ReadByte()
        if err != nil {{
            if i > 0 && err == {io}.EOF {{
                err = {io}.ErrUnexpectedEOF
            }}
            return x, {fmt}.Errorf("failed to read u32 byte: %w", err)
        }}
        if s == 28 && b > 0x0f {{
            return x, {errors}.New("varint overflows a 32-bit integer")
        }}
        if b < 0x80 {{
            return x | uint32(b)<<s, nil
        }}
        x |= uint32(b&0x7f) << s
        s += 7
    }}
    return x, {errors}.New("varint overflows a 32-bit integer")
}}({reader})"#,
        );
    }

    fn print_read_u64(&mut self, reader: &str) {
        let fmt = self.deps.fmt();
        let errors = self.deps.errors();
        let io = self.deps.io();
        let slog = self.deps.slog();
        uwrite!(
            self.src,
            r#"func(r {io}.ByteReader) (uint64, error) {{
    var x uint64
    var s uint8
    for i := 0; i < 10; i++ {{
        {slog}.Debug("reading u64 byte", "i", i)
        b, err := r.ReadByte()
        if err != nil {{
            if i > 0 && err == {io}.EOF {{
                err = {io}.ErrUnexpectedEOF
            }}
            return x, {fmt}.Errorf("failed to read u64 byte: %w", err)
        }}
        if s == 63 && b > 0x01 {{
            return x, {errors}.New("varint overflows a 64-bit integer")
        }}
        if b < 0x80 {{
            return x | uint64(b)<<s, nil
        }}
        x |= uint64(b&0x7f) << s
        s += 7
    }}
    return x, {errors}.New("varint overflows a 64-bit integer")
}}({reader})"#,
        );
    }

    fn print_read_s8(&mut self, reader: &str) {
        let fmt = self.deps.fmt();
        let io = self.deps.io();
        let slog = self.deps.slog();
        uwrite!(
            self.src,
            r#"func(r {io}.ByteReader) (int8, error) {{
    {slog}.Debug("reading s8 byte")
    v, err := r.ReadByte()
    if err != nil {{
        return 0, {fmt}.Errorf("failed to read s8 byte: %w", err)
    }}
    return int8(v), nil
}}({reader})"#,
        );
    }

    fn print_read_s16(&mut self, reader: &str) {
        let fmt = self.deps.fmt();
        let io = self.deps.io();
        let slog = self.deps.slog();
        let errors = self.deps.errors();
        uwrite!(
            self.src,
            r#"func(r {io}.ByteReader) (int16, error) {{
    var (
        b          byte
        err        error
        result     int16
        shift      uint16
        length     uint32
        endReached bool
    )
    const lenCeiling = 3
    for i := 0; i < lenCeiling; i++ {{
        {slog}.Debug("reading s16 byte")
        b, err = r.ReadByte()
        if err != nil {{
            return 0, {fmt}.Errorf("failed to read s16 byte: %w", err)
        }}
        length++

        result |= (int16(b) & 0x7f) << shift
        shift += 7
        if b&0x80 == 0 {{
            endReached = true
            break
        }}
    }}
    if !endReached {{
        return 0, {errors}.New("cannot decode s16 past the length limit")
    }}
    if (shift < 8*uint16(length)) && (b&0x40 > 0) {{
        result |= -(1 << shift)
    }}
    return result, nil
}}({reader})"#,
        );
    }

    fn print_read_s32(&mut self, reader: &str) {
        let fmt = self.deps.fmt();
        let io = self.deps.io();
        let slog = self.deps.slog();
        let errors = self.deps.errors();
        uwrite!(
            self.src,
            r#"func(r {io}.ByteReader) (int32, error) {{
    var (
        b          byte
        err        error
        result     int32
        shift      uint32
        length     uint32
        endReached bool
    )
    const lenCeiling = 5
    for i := 0; i < lenCeiling; i++ {{
        {slog}.Debug("reading s32 byte")
        b, err = r.ReadByte()
        if err != nil {{
            return 0, {fmt}.Errorf("failed to read s32 byte: %w", err)
        }}
        length++

        result |= (int32(b) & 0x7f) << shift
        shift += 7
        if b&0x80 == 0 {{
            endReached = true
            break
        }}
    }}
    if !endReached {{
        return 0, {errors}.New("cannot decode s32 past the length limit")
    }}
    if (shift < 8*uint32(length)) && (b&0x40 > 0) {{
        result |= -(1 << shift)
    }}
    return result, nil
}}({reader})"#,
        );
    }

    fn print_read_s64(&mut self, reader: &str) {
        let fmt = self.deps.fmt();
        let io = self.deps.io();
        let slog = self.deps.slog();
        let errors = self.deps.errors();
        uwrite!(
            self.src,
            r#"func(r {io}.ByteReader) (int64, error) {{
    var (
        b          byte
        err        error
        result     int64
        shift      uint64
        length     uint32
        endReached bool
    )
    const lenCeiling = 10
    for i := 0; i < lenCeiling; i++ {{
        {slog}.Debug("reading s64 byte")
        b, err = r.ReadByte()
        if err != nil {{
            return 0, {fmt}.Errorf("failed to read s64 byte: %w", err)
        }}
        length++

        result |= (int64(b) & 0x7f) << shift
        shift += 7
        if b&0x80 == 0 {{
            endReached = true
            break
        }}
    }}
    if !endReached {{
        return 0, {errors}.New("cannot decode s64 past the length limit")
    }}
    if (shift < 8*uint64(length)) && (b&0x40 > 0) {{
        result |= -(1 << shift)
    }}
    return result, nil
}}({reader})"#,
        );
    }

    fn print_read_f32(&mut self, reader: &str) {
        let binary = self.deps.binary();
        let fmt = self.deps.fmt();
        let io = self.deps.io();
        let math = self.deps.math();
        let slog = self.deps.slog();
        uwrite!(
            self.src,
            r#"func(r {io}.Reader) (float32, error) {{
    var b [4]byte
    {slog}.Debug("reading f32 bytes")
    if _, err := {io}.ReadFull(r, b[:]); err != nil {{
        return 0, {fmt}.Errorf("failed to read f32: %w", err)
    }}
    return {math}.Float32frombits({binary}.LittleEndian.Uint32(b[:])), nil
}}({reader})"#,
        );
    }

    fn print_read_f64(&mut self, reader: &str) {
        let binary = self.deps.binary();
        let fmt = self.deps.fmt();
        let io = self.deps.io();
        let math = self.deps.math();
        let slog = self.deps.slog();
        uwrite!(
            self.src,
            r#"func(r {io}.Reader) (float64, error) {{
    var b [8]byte
    {slog}.Debug("reading f64 bytes")
    if _, err := {io}.ReadFull(r, b[:]); err != nil {{
        return 0, {fmt}.Errorf("failed to read f64: %w", err)
    }}
    return {math}.Float64frombits({binary}.LittleEndian.Uint64(b[:])), nil
}}({reader})"#,
        );
    }

    fn print_read_char(&mut self, reader: &str) {
        let errors = self.deps.errors();
        let fmt = self.deps.fmt();
        let io = self.deps.io();
        let slog = self.deps.slog();
        let utf8 = self.deps.utf8();
        uwrite!(
            self.src,
            r#"func(r {io}.ByteReader) (rune, error) {{
    var x uint32
    var s uint8
    for i := 0; i < 5; i++ {{
        {slog}.Debug("reading char byte", "i", i)
        b, err := r.ReadByte()
        if err != nil {{
            if i > 0 && err == {io}.EOF {{
                err = {io}.ErrUnexpectedEOF
            }}
            return {utf8}.RuneError, {fmt}.Errorf("failed to read char byte: %w", err)
        }}
        if s == 28 && b > 0x0f {{
            return {utf8}.RuneError, {errors}.New("char overflows a 32-bit integer")
        }}
        if b < 0x80 {{
            x = x | uint32(b)<<s
            v := rune(x)
            if !{utf8}.ValidRune(v) {{
                return v, {errors}.New("char is not valid UTF-8")
            }}
            return v, nil
        }}
        x |= uint32(b&0x7f) << s
        s += 7
    }}
    return {utf8}.RuneError, {errors}.New("char overflows a 32-bit integer")
}}({reader})"#,
        );
    }

    fn print_read_string(&mut self, reader: &str) {
        let errors = self.deps.errors();
        let fmt = self.deps.fmt();
        let io = self.deps.io();
        let slog = self.deps.slog();
        let wrpc = self.deps.wrpc();
        let utf8 = self.deps.utf8();
        uwrite!(
            self.src,
            r#"func(r interface {{ {io}.ByteReader; {io}.Reader }}) (string, error) {{
    var x uint32
    var s uint8
    for i := 0; i < 5; i++ {{
        {slog}.Debug("reading string length byte", "i", i)
        b, err := r.ReadByte()
        if err != nil {{
            if i > 0 && err == {io}.EOF {{
                err = {io}.ErrUnexpectedEOF
            }}
            return "", {fmt}.Errorf("failed to read string length byte: %w", err)
        }}
        if s == 28 && b > 0x0f {{
            return "", {errors}.New("string length overflows a 32-bit integer")
        }}
        if b < 0x80 {{
            x = x | uint32(b)<<s
            if x == 0 {{
                return "", nil
            }}
            {slog}.Debug("reading string bytes", "len", x)
            buf, err := {wrpc}.ReadBytes(r, x)
            if err != nil {{
                return "", {fmt}.Errorf("failed to read string bytes: %w", err)
            }}
            if !{utf8}.Valid(buf) {{
                return string(buf), {errors}.New("string is not valid UTF-8")
            }}
            return string(buf), nil
        }}
        x |= uint32(b&0x7f) << s
        s += 7
    }}
    return "", {errors}.New("string length overflows a 32-bit integer")
}}({reader})"#,
        );
    }

    fn print_read_byte_list(&mut self, reader: &str) {
        let errors = self.deps.errors();
        let fmt = self.deps.fmt();
        let io = self.deps.io();
        let slog = self.deps.slog();
        let wrpc = self.deps.wrpc();
        uwrite!(
            self.src,
            r#"func(r interface {{ {io}.ByteReader; {io}.Reader }}) ([]byte, error) {{
    var x uint32
    var s uint
    for i := 0; i < 5; i++ {{
        {slog}.Debug("reading byte list length", "i", i)
        b, err := r.ReadByte()
        if err != nil {{
            if i > 0 && err == {io}.EOF {{
                err = {io}.ErrUnexpectedEOF
            }}
            return nil, {fmt}.Errorf("failed to read byte list length byte: %w", err)
        }}
        if s == 28 && b > 0x0f {{
            return nil, {errors}.New("byte list length overflows a 32-bit integer")
        }}
        if b < 0x80 {{
            x = x | uint32(b)<<s
            if x == 0 {{
                return nil, nil
            }}
            {slog}.Debug("reading byte list contents", "len", x)
            buf, err := {wrpc}.ReadBytes(r, x)
            if err != nil {{
                return nil, {fmt}.Errorf("failed to read byte list contents: %w", err)
            }}
            return buf, nil
        }}
        x |= uint32(b&0x7f) << s
        s += 7
    }}
    return nil, {errors}.New("byte length overflows a 32-bit integer")
}}({reader})"#,
        );
    }

    fn print_read_list(&mut self, ty: &Type, reader: &str, path: &str) {
        if is_ty(self.resolve, Type::U8, ty) {
            self.print_read_byte_list(reader);
            return;
        }
        let fmt = self.deps.fmt();
        let io = self.deps.io();
        let errors = self.deps.errors();
        let slog = self.deps.slog();
        let wrpc = self.deps.wrpc();
        let deferred = self.is_async(ty);
        let d = if deferred { "nil, " } else { "" };
        uwrite!(self.src, "func(r {wrpc}.IndexReadCloser, path ...uint32) (");
        self.print_list(ty);
        self.print_deferred_ret(deferred);
        uwrite!(
            self.src,
            r#", error) {{
    var x uint32
    var s uint
    for i := 0; i < 5; i++ {{
        {slog}.Debug("reading list length byte", "i", i)
        b, err := r.ReadByte()
        if err != nil {{
            if i > 0 && err == {io}.EOF {{
                err = {io}.ErrUnexpectedEOF
            }}
            return nil, {d}{fmt}.Errorf("failed to read list length byte: %w", err)
        }}
        if s == 28 && b > 0x0f {{
            return nil, {d}{errors}.New("list length overflows a 32-bit integer")
        }}
        if b < 0x80 {{
            x = x | uint32(b)<<s
            if x == 0 {{
                return nil, {d}nil
            }}
            vs := {wrpc}.NewSlice["#,
        );
        self.print_list(ty);
        uwrite!(self.src, "](x)\n");
        if deferred {
            let context = self.deps.context();
            uwrite!(self.src, "var ds []func({context}.Context) error\n");
        }
        uwrite!(
            self.src,
            r#"for i := range x {{
                {slog}.Debug("reading list element", "i", i)
                v, "#,
        );
        if deferred {
            self.push_str("d, ");
        }
        self.push_str("err := ");
        self.print_read_ty(ty, "r", "append(path, uint32(i))");
        self.push_str("\n");
        uwrite!(
            self.src,
            r#"if err != nil {{
                    return nil, {d}{fmt}.Errorf("failed to read list element %d: %w", i, err)
                }}
                vs = append(vs, v)
"#,
        );
        if deferred {
            uwrite!(
                self.src,
                r#"ds = append(ds, d)
            }}
            return vs, {wrpc}.JoinDeferred(ds...), nil
"#
            );
        } else {
            self.push_str("}\nreturn vs, nil\n");
        }
        uwrite!(
            self.src,
            r#"}}
        x |= uint32(b&0x7f) << s
        s += 7
    }}
    return nil, {d}{errors}.New("list length overflows a 32-bit integer")
}}({reader}"#,
        );
        if !path.is_empty() {
            self.src.push_str(", ");
            self.src.push_str(path);
            self.src.push_str("...");
        }
        self.src.push_str(")");
    }

    fn print_read_option(&mut self, ty: &Type, reader: &str, path: &str) {
        let fmt = self.deps.fmt();
        let slog = self.deps.slog();
        let wrpc = self.deps.wrpc();
        let deferred = self.is_async(ty);
        let (d, dv) = if deferred { ("nil, ", "d, ") } else { ("", "") };
        uwrite!(self.src, "func(r {wrpc}.IndexReadCloser, path ...uint32) (");
        self.print_option(ty, true);
        self.print_deferred_ret(deferred);
        uwrite!(
            self.src,
            r#", error) {{
    {slog}.Debug("reading option status byte")
    status, err := r.ReadByte()
    if err != nil {{
        return nil, {d}{fmt}.Errorf("failed to read option status byte: %w", err)
    }}
    switch status {{
    case 0:
        return nil, {d}nil
    case 1:
        {slog}.Debug("reading `option::some` payload")
        v, {dv}err := "#,
        );
        self.print_read_ty(ty, "r", "path");
        self.push_str("\n");
        uwrite!(
            self.src,
            r#"if err != nil {{
            return nil, {d}{fmt}.Errorf("failed to read `option::some` value: %w", err)
        }}
        return "#,
        );
        self.print_nillable_ptr(ty, false, false);
        uwrite!(
            self.src,
            r#"v, {dv}nil
    default:
        return nil, {d}{fmt}.Errorf("invalid option status byte %d", status)
    }}
}}({reader}"#,
        );
        if !path.is_empty() {
            self.src.push_str(", ");
            self.src.push_str(path);
            self.src.push_str("...");
        }
        self.src.push_str(")");
    }

    fn print_read_result(&mut self, ty: &Result_, reader: &str, path: &str) {
        let fmt = self.deps.fmt();
        let slog = self.deps.slog();
        let wrpc = self.deps.wrpc();
        let deferred = ty.ok.iter().chain(&ty.err).any(|ty| self.is_async(ty));
        let d = if deferred { "nil, " } else { "" };
        uwrite!(
            self.src,
            "func(r {wrpc}.IndexReadCloser, path ...uint32) (*"
        );
        self.print_result(ty);
        self.print_deferred_ret(deferred);
        uwriteln!(
            self.src,
            r#", error) {{
    {slog}.Debug("reading result status byte")
    status, err := r.ReadByte()
    if err != nil {{
        return nil, {d}{fmt}.Errorf("failed to read result status byte: %w", err)
    }}"#,
        );
        self.push_str("switch status {\n");
        for (i, (case, field, payload)) in [("ok", "Ok", &ty.ok), ("err", "Err", &ty.err)]
            .into_iter()
            .enumerate()
        {
            uwriteln!(self.src, "case {i}:");
            let mut ret = d;
            if let Some(payload) = payload {
                uwriteln!(
                    self.src,
                    r#"{slog}.Debug("reading `result::{case}` payload")"#
                );
                if self.is_async(payload) {
                    self.push_str("v, d, err := ");
                    ret = "d, ";
                } else {
                    self.push_str("v, err := ");
                }
                self.print_read_ty(payload, "r", "path");
                self.push_str("\n");
                uwriteln!(
                    self.src,
                    r#"if err != nil {{
            return nil, {d}{fmt}.Errorf("failed to read `result::{case}` value: %w", err)
        }}"#,
                );
                self.push_str("return &");
                self.print_result(ty);
                uwrite!(self.src, "{{ {field}: ");
                self.print_nillable_ptr(payload, true, false);
            } else {
                self.push_str("var v struct{}\n");
                self.push_str("return &");
                self.print_result(ty);
                uwrite!(self.src, "{{ {field}: &");
            }
            uwriteln!(self.src, "v }}, {ret}nil");
        }
        uwrite!(
            self.src,
            r#"default:
        return nil, {d}{fmt}.Errorf("invalid result status byte %d", status)
    }}
}}({reader}"#
        );
        if !path.is_empty() {
            self.src.push_str(", ");
            self.src.push_str(path);
            self.src.push_str("...");
        }
        self.src.push_str(")");
    }

    fn print_read_record(
        &mut self,
        Record { fields, .. }: &Record,
        reader: &str,
        path: &str,
        name: &str,
    ) {
        let wrpc = self.deps.wrpc();
        let deferred = fields.iter().any(|Field { ty, .. }| self.is_async(ty));
        let d = if deferred { "nil, " } else { "" };
        uwrite!(
            self.src,
            "func(r {wrpc}.IndexReadCloser, path ...uint32) (*{name}"
        );
        self.print_deferred_ret(deferred);
        uwriteln!(
            self.src,
            r#", error) {{
    v := &{name}{{}}
    var err error"#
        );
        if deferred {
            let context = self.deps.context();
            uwrite!(
                self.src,
                "var d func({context}.Context) error\nvar ds []func({context}.Context) error\n"
            );
        }
        for (i, Field { name, ty, .. }) in fields.iter().enumerate() {
            let fmt = self.deps.fmt();
            let slog = self.deps.slog();
            let ident = name.to_upper_camel_case();
            let field_deferred = self.is_async(ty);
            uwrite!(
                self.src,
                r#"{slog}.Debug("reading field", "name", "{name}")
    v.{ident}, "#
            );
            if field_deferred {
                self.push_str("d, ");
            }
            self.push_str("err = ");
            self.print_read_ty(ty, "r", &format!("append(path, {i})"));
            self.push_str("\n");
            uwriteln!(
                self.src,
                r#"if err != nil {{
            return nil, {d}{fmt}.Errorf("failed to read `{name}` field: %w", err)
        }}"#
            );
            if field_deferred {
                self.push_str("ds = append(ds, d)\n");
            }
        }
        if deferred {
            uwriteln!(self.src, "return v, {wrpc}.JoinDeferred(ds...), nil");
        } else {
            self.push_str("return v, nil\n");
        }
        uwrite!(self.src, "}}({reader}");
        if !path.is_empty() {
            self.src.push_str(", ");
            self.src.push_str(path);
            self.src.push_str("...");
        }
        self.src.push_str(")");
    }

    fn print_read_flags(&mut self, reader: &str, name: &str) {
        let wrpc = self.deps.wrpc();

        uwrite!(
            self.src,
            r#"func(r {wrpc}.IndexReader) (*{name}, error) {{
    v := {name}{{}}
    if err := v.ReadFromIndex(r); err != nil {{
        return nil, err
    }}
    return &v, nil
}}({reader})
"#
        );
    }

    fn print_read_enum(&mut self, ty: &Enum, reader: &str, name: &str) {
        let fmt = self.deps.fmt();
        let io = self.deps.io();

        uwrite!(
            self.src,
            r#"func(r {io}.ByteReader) (v {name}, err error) {{
    n, err := "#
        );
        self.print_read_discriminant(ty.tag(), "r");
        self.push_str("\n");
        self.push_str("if err != nil {\n");
        self.push_str("return v, ");
        self.push_str(fmt);
        self.push_str(".Errorf(\"failed to read discriminant: %w\", err)\n}\n");
        uwriteln!(self.src, "switch {name}(n) {{");
        for EnumCase {
            name: case_name, ..
        } in &ty.cases
        {
            self.push_str("case ");
            self.push_str(name);
            self.push_str("_");
            self.push_str(&case_name.to_upper_camel_case());
            self.push_str(":\n");
            self.push_str("return ");
            self.push_str(name);
            self.push_str("_");
            self.push_str(&case_name.to_upper_camel_case());
            self.push_str(", nil\n");
        }
        uwriteln!(
            self.src,
            r#"default: return v, {fmt}.Errorf("unknown discriminant value %d", n) }}"#
        );
        uwrite!(self.src, "}}({reader})");
    }

    fn print_read_variant(&mut self, ty: &Variant, reader: &str, path: &str, name: &str) {
        let fmt = self.deps.fmt();
        let wrpc = self.deps.wrpc();
        let deferred = ty
            .cases
            .iter()
            .any(|Case { ty, .. }| ty.as_ref().is_some_and(|ty| self.is_async(ty)));
        let d = if deferred { "nil, " } else { "" };
        uwrite!(
            self.src,
            "func(r {wrpc}.IndexReadCloser, path ...uint32) (*{name}"
        );
        self.print_deferred_ret(deferred);
        uwrite!(
            self.src,
            r#", error) {{
    v := &{name}{{}}
    n, err := "#
        );
        self.print_read_discriminant(ty.tag(), "r");
        self.push_str("\n");
        uwriteln!(
            self.src,
            r#"if err != nil {{
        return nil, {d}{fmt}.Errorf("failed to read discriminant: %w", err)
    }}"#
        );
        uwriteln!(self.src, "switch {name}Discriminant(n) {{");
        for Case {
            name: case_name,
            ty,
            ..
        } in &ty.cases
        {
            let camel = case_name.to_upper_camel_case();
            uwriteln!(self.src, "case {name}{camel}:");
            if let Some(ty) = ty {
                let ret = if self.is_async(ty) {
                    self.push_str("payload, d, err := ");
                    "d, "
                } else {
                    self.push_str("payload, err := ");
                    d
                };
                self.print_read_ty(ty, "r", "path");
                self.push_str("\n");
                uwriteln!(
                    self.src,
                    r#"if err != nil {{ return nil, {d}{fmt}.Errorf("failed to read `{case_name}` payload: %w", err) }}"#
                );
                uwriteln!(self.src, "return v.Set{camel}(payload), {ret}nil");
            } else {
                uwriteln!(self.src, "return v.Set{camel}(), {d}nil");
            }
        }
        uwriteln!(
            self.src,
            r#"default: return nil, {d}{fmt}.Errorf("unknown discriminant value %d", n) }}"#
        );
        uwrite!(self.src, "}}({reader}");
        if !path.is_empty() {
            self.src.push_str(", ");
            self.src.push_str(path);
            self.src.push_str("...");
        }
        self.src.push_str(")");
    }

    fn print_read_tuple(&mut self, ty: &Tuple, reader: &str, path: &str) {
        match ty.types.as_slice() {
            [] => self.push_str("struct{}{}, nil"),
            [ty] => {
                if path.is_empty() {
                    self.print_read_ty(ty, reader, "0");
                } else {
                    self.print_read_ty(ty, reader, &format!("append({path}, 0)"));
                }
            }
            _ => {
                let wrpc = self.deps.wrpc();
                let deferred = ty.types.iter().any(|ty| self.is_async(ty));
                let d = if deferred { "nil, " } else { "" };
                uwrite!(
                    self.src,
                    "func(r {wrpc}.IndexReadCloser, path ...uint32) (*"
                );
                self.print_tuple(ty, true);
                self.print_deferred_ret(deferred);
                self.push_str(", error) {\n");
                self.push_str("v := &");
                self.print_tuple(ty, false);
                self.push_str("{}\n");
                self.push_str("var err error\n");
                if deferred {
                    let context = self.deps.context();
                    uwrite!(
                        self.src,
                        "var d func({context}.Context) error\nvar ds []func({context}.Context) error\n"
                    );
                }
                for (i, ty) in ty.types.iter().enumerate() {
                    let fmt = self.deps.fmt();
                    let slog = self.deps.slog();
                    let elem_deferred = self.is_async(ty);
                    uwrite!(
                        self.src,
                        r#"{slog}.Debug("reading tuple element {i}")
        v.V{i}, "#
                    );
                    if elem_deferred {
                        self.push_str("d, ");
                    }
                    self.push_str("err = ");
                    self.print_read_ty(ty, "r", &format!("append(path, {i})"));
                    self.push_str("\n");
                    uwriteln!(
                        self.src,
                        r#"if err != nil {{
            return nil, {d}{fmt}.Errorf("failed to read tuple element {i}: %w", err)
        }}"#
                    );
                    if elem_deferred {
                        self.push_str("ds = append(ds, d)\n");
                    }
                }
                if deferred {
                    uwriteln!(self.src, "return v, {wrpc}.JoinDeferred(ds...), nil");
                } else {
                    self.push_str("return v, nil\n");
                }
                uwrite!(self.src, "}}({reader}");
                if !path.is_empty() {
                    self.src.push_str(", ");
                    self.src.push_str(path);
                    self.src.push_str("...");
                }
                self.src.push_str(")");
            }
        }
    }

    fn print_read_future(&mut self, ty: &Option<Type>, reader: &str, path: &str) {
        let wrpc = self.deps.wrpc();
        if let Some(ty) = ty {
            uwrite!(self.src, "{wrpc}.ReadFuture({reader}, ");
            self.print_read_element(ty);
        } else {
            uwrite!(self.src, "{wrpc}.ReadFutureUnit({reader}");
        }
        if !path.is_empty() {
            self.src.push_str(", ");
            self.src.push_str(path);
            self.src.push_str("...");
        }
        self.src.push_str(")");
    }

    fn print_read_element(&mut self, ty: &Type) {
        let wrpc = self.deps.wrpc();
        uwrite!(self.src, "func(r {wrpc}.IndexReadCloser, path ...uint32) (");
        self.print_opt_ty(ty, true);
        let context = self.deps.context();
        uwrite!(self.src, ", func({context}.Context) error, error) {{\n");
        if self.is_async(ty) {
            self.push_str("return ");
            self.print_read_ty(ty, "r", "path");
            self.push_str("\n");
        } else {
            self.push_str("v, err := ");
            self.print_read_ty(ty, "r", "path");
            self.push_str("\nreturn v, nil, err\n");
        }
        self.push_str("}");
    }

    fn print_read_stream(&mut self, element: &Option<Type>, reader: &str, path: &str) {
        let wrpc = self.deps.wrpc();
        match element {
            Some(ty) if is_ty(self.resolve, Type::U8, ty) => {
                uwrite!(self.src, "{wrpc}.ReadByteStream({reader}");
            }
            Some(ty) => {
                uwrite!(self.src, "{wrpc}.ReadStream({reader}, ");
                self.print_read_element(ty);
            }
            None => panic!("streams with no element types are not supported"),
        }
        if !path.is_empty() {
            self.src.push_str(", ");
            self.src.push_str(path);
            self.src.push_str("...");
        }
        self.src.push_str(")");
    }

    fn print_read_own(&mut self, reader: &str, id: TypeId) {
        let errors = self.deps.errors();
        let fmt = self.deps.fmt();
        let io = self.deps.io();
        let slog = self.deps.slog();
        let wrpc = self.deps.wrpc();
        uwrite!(
            self.src,
            "func(r interface {{ {io}.ByteReader; {io}.Reader }}) (",
        );
        self.print_own(id);
        uwrite!(
            self.src,
            r#", error) {{
    var x uint32
    var s uint
    for i := 0; i < 5; i++ {{
        {slog}.Debug("reading owned resource handle length byte", "i", i)
        b, err := r.ReadByte()
        if err != nil {{
            if i > 0 && err == {io}.EOF {{
                err = {io}.ErrUnexpectedEOF
            }}
            return nil, {fmt}.Errorf("failed to read owned resource handle length byte: %w", err)
        }}
        if b < 0x80 {{
            if i == 4 && b > 1 {{
                return nil, {errors}.New("owned resource handle length overflows a 32-bit integer")
            }}
            x = x | uint32(b)<<s
            {slog}.Debug("reading owned resource handle bytes", "len", x)
            buf, err := {wrpc}.ReadBytes(r, x)
            if err != nil {{
                return nil, {fmt}.Errorf("failed to read owned resource handle bytes: %w", err)
            }}
            return "#,
        );
        self.print_own(id);
        uwrite!(
            self.src,
            r#"(buf), nil
        }}
        x |= uint32(b&0x7f) << s
        s += 7
    }}
    return nil, {errors}.New("owned resource handle length overflows a 32-bit integer")
}}({reader})"#,
        );
    }

    fn print_read_borrow(&mut self, reader: &str, id: TypeId) {
        let errors = self.deps.errors();
        let fmt = self.deps.fmt();
        let io = self.deps.io();
        let slog = self.deps.slog();
        let wrpc = self.deps.wrpc();
        uwrite!(
            self.src,
            "func(r interface {{ {io}.ByteReader; {io}.Reader }}) (",
        );
        self.print_borrow(id);
        uwrite!(
            self.src,
            r#", error) {{
    var x uint32
    var s uint
    for i := 0; i < 5; i++ {{
        {slog}.Debug("reading borrowed resource handle length byte", "i", i)
        b, err := r.ReadByte()
        if err != nil {{
            if i > 0 && err == {io}.EOF {{
                err = {io}.ErrUnexpectedEOF
            }}
            return nil, {fmt}.Errorf("failed to read borrowed resource handle length byte: %w", err)
        }}
        if b < 0x80 {{
            if i == 4 && b > 1 {{
                return nil, {errors}.New("borrowed resource handle length overflows a 32-bit integer")
            }}
            x = x | uint32(b)<<s
            {slog}.Debug("reading borrowed resource handle bytes", "len", x)
            buf, err := {wrpc}.ReadBytes(r, x)
            if err != nil {{
                return nil, {fmt}.Errorf("failed to read borrowed resource handle bytes: %w", err)
            }}
            return "#,
        );
        self.print_borrow(id);
        uwrite!(
            self.src,
            r#"(buf), nil
        }}
        x |= uint32(b&0x7f) << s
        s += 7
    }}
    return nil, {errors}.New("borrowed resource handle length overflows a 32-bit integer")
}}({reader})"#,
        );
    }

    fn print_read_ty(&mut self, ty: &Type, reader: &str, path: &str) {
        match ty {
            Type::Id(ty) => self.print_read_tyid(*ty, reader, path),
            Type::Bool => self.print_read_bool(reader),
            Type::U8 => self.print_read_u8(reader),
            Type::U16 => self.print_read_u16(reader),
            Type::U32 => self.print_read_u32(reader),
            Type::U64 => self.print_read_u64(reader),
            Type::S8 => self.print_read_s8(reader),
            Type::S16 => self.print_read_s16(reader),
            Type::S32 => self.print_read_s32(reader),
            Type::S64 => self.print_read_s64(reader),
            Type::F32 => self.print_read_f32(reader),
            Type::F64 => self.print_read_f64(reader),
            Type::Char => self.print_read_char(reader),
            Type::String => self.print_read_string(reader),
            Type::ErrorContext => panic!("error-context is not supported"),
        }
    }

    fn print_read_tyid(&mut self, id: TypeId, reader: &str, path: &str) {
        let ty = &self.resolve.types[id];
        let name = ty
            .name
            .as_ref()
            .map(|name| self.type_path_with_name(id, to_upper_camel_case(name)));
        match &ty.kind {
            TypeDefKind::Record(ty) => {
                self.print_read_record(ty, reader, path, &name.expect("record missing a name"));
            }
            TypeDefKind::Resource => self.print_read_string(reader),
            TypeDefKind::Handle(Handle::Own(id)) => self.print_read_own(reader, *id),
            TypeDefKind::Handle(Handle::Borrow(id)) => self.print_read_borrow(reader, *id),
            TypeDefKind::Flags(_ty) => {
                self.print_read_flags(reader, &name.expect("flag missing a name"));
            }
            TypeDefKind::Tuple(ty) => self.print_read_tuple(ty, reader, path),
            TypeDefKind::Variant(ty) => {
                self.print_read_variant(ty, reader, path, &name.expect("variant missing a name"));
            }
            TypeDefKind::Enum(ty) => {
                self.print_read_enum(ty, reader, &name.expect("enum missing a name"));
            }
            TypeDefKind::Option(ty) => self.print_read_option(ty, reader, path),
            TypeDefKind::Result(ty) => self.print_read_result(ty, reader, path),
            TypeDefKind::List(ty) => self.print_read_list(ty, reader, path),
            TypeDefKind::FixedLengthList(..) => panic!("unsupported type: fixed size list"),
            TypeDefKind::Map(..) => panic!("unsupported type: map"),
            TypeDefKind::Future(ty) => self.print_read_future(ty, reader, path),
            TypeDefKind::Stream(ty) => self.print_read_stream(ty, reader, path),
            TypeDefKind::Type(ty) => {
                if let Some(name) = name {
                    let deferred = self.is_async(ty);
                    self.push_str("func() (");
                    self.print_opt_ptr(ty, true);
                    self.push_str(&name);
                    self.print_deferred_ret(deferred);
                    self.push_str(", error) {\n");
                    if deferred {
                        self.push_str("v, d, err :=");
                    } else {
                        self.push_str("v, err :=");
                    }
                    self.print_read_ty(ty, reader, path);
                    self.push_str("\n");
                    self.push_str("return (");
                    self.print_opt_ptr(ty, true);
                    self.push_str(&name);
                    if deferred {
                        self.push_str(")(v), d, err\n");
                    } else {
                        self.push_str(")(v), err\n");
                    }
                    self.push_str("}()\n");
                } else {
                    self.print_read_ty(ty, reader, path);
                }
            }
            TypeDefKind::Unknown => unreachable!(),
        }
    }

    fn print_write_bool(&mut self, name: &str, writer: &str) {
        let io = self.deps.io();
        let slog = self.deps.slog();
        uwrite!(
            self.src,
            r#"func(v bool, w {io}.ByteWriter) error {{
                if !v {{
                    {slog}.Debug("writing `false` byte")
                    return w.WriteByte(0)
                }}
                {slog}.Debug("writing `true` byte")
                return w.WriteByte(1)
            }}({name}, {writer})"#,
        );
    }

    fn print_write_u8(&mut self, name: &str, writer: &str) {
        let io = self.deps.io();
        let slog = self.deps.slog();
        uwrite!(
            self.src,
            r#"func(v uint8, w {io}.ByteWriter) error {{
                {slog}.Debug("writing u8 byte")
                return w.WriteByte(v)
            }}({name}, {writer})"#,
        );
    }

    fn print_write_u16(&mut self, name: &str, writer: &str) {
        let binary = self.deps.binary();
        let io = self.deps.io();
        let slog = self.deps.slog();
        uwrite!(
            self.src,
            r#"func(v uint16, w {io}.Writer) (err error) {{
                b := make([]byte, {binary}.MaxVarintLen16)
                i := {binary}.PutUvarint(b, uint64(v))
                {slog}.Debug("writing u16")
                _, err = w.Write(b[:i])
                return err
            }}({name}, {writer})"#,
        );
    }

    fn print_write_u32(&mut self, name: &str, writer: &str) {
        let binary = self.deps.binary();
        let io = self.deps.io();
        let slog = self.deps.slog();
        uwrite!(
            self.src,
            r#"func(v uint32, w {io}.Writer) (err error) {{
                b := make([]byte, {binary}.MaxVarintLen32)
                i := {binary}.PutUvarint(b, uint64(v))
                {slog}.Debug("writing u32")
                _, err = w.Write(b[:i])
                return err
            }}({name}, {writer})"#,
        );
    }

    fn print_write_u64(&mut self, name: &str, writer: &str) {
        let binary = self.deps.binary();
        let io = self.deps.io();
        let slog = self.deps.slog();
        uwrite!(
            self.src,
            r#"func(v uint64, w {io}.Writer) (err error) {{
                b := make([]byte, {binary}.MaxVarintLen64)
                i := {binary}.PutUvarint(b, uint64(v))
                {slog}.Debug("writing u64")
                _, err = w.Write(b[:i])
                return err
            }}({name}, {writer})"#,
        );
    }

    fn print_write_s8(&mut self, name: &str, writer: &str) {
        let io = self.deps.io();
        let slog = self.deps.slog();
        uwrite!(
            self.src,
            r#"func(v int8, w {io}.ByteWriter) error {{
                {slog}.Debug("writing s8 byte")
                return w.WriteByte(byte(v))
            }}({name}, {writer})"#,
        );
    }

    fn print_write_s16(&mut self, name: &str, writer: &str) {
        let io = self.deps.io();
        let fmt = self.deps.fmt();
        let slog = self.deps.slog();
        uwrite!(
            self.src,
            r#"func(v int16, w {io}.ByteWriter) (err error) {{
                for {{
                    b := byte(v & 0x7f)
                    v >>= 7

                    signb := b & 0x40

                    last := false
                    if (v == 0 && signb == 0) || (v == -1 && signb != 0) {{
                        last = true
                    }} else {{
                        b = b | 0x80
                    }}
                    {slog}.Debug("writing s16 byte")
                    if err = w.WriteByte(b); err != nil {{
                        return {fmt}.Errorf("failed to write `s16` byte: %w", err)
                    }}
                    if last {{
                        return nil
                    }}
                }}
            }}({name}, {writer})"#,
        );
    }

    fn print_write_s32(&mut self, name: &str, writer: &str) {
        let io = self.deps.io();
        let fmt = self.deps.fmt();
        let slog = self.deps.slog();
        uwrite!(
            self.src,
            r#"func(v int32, w {io}.ByteWriter) (err error) {{
                for {{
                    b := byte(v & 0x7f)
                    v >>= 7

                    signb := b & 0x40

                    last := false
                    if (v == 0 && signb == 0) || (v == -1 && signb != 0) {{
                        last = true
                    }} else {{
                        b = b | 0x80
                    }}
                    {slog}.Debug("writing s32 byte")
                    if err = w.WriteByte(b); err != nil {{
                        return {fmt}.Errorf("failed to write `s32` byte: %w", err)
                    }}
                    if last {{
                        return nil
                    }}
                }}
            }}({name}, {writer})"#,
        );
    }

    fn print_write_s64(&mut self, name: &str, writer: &str) {
        let io = self.deps.io();
        let fmt = self.deps.fmt();
        let slog = self.deps.slog();
        uwrite!(
            self.src,
            r#"func(v int64, w {io}.ByteWriter) (err error) {{
                for {{
                    b := byte(v & 0x7f)
                    v >>= 7

                    signb := b & 0x40

                    last := false
                    if (v == 0 && signb == 0) || (v == -1 && signb != 0) {{
                        last = true
                    }} else {{
                        b = b | 0x80
                    }}
                    {slog}.Debug("writing s64 byte")
                    if err = w.WriteByte(b); err != nil {{
                        return {fmt}.Errorf("failed to write `s64` byte: %w", err)
                    }}
                    if last {{
                        return nil
                    }}
                }}
            }}({name}, {writer})"#,
        );
    }

    fn print_write_f32(&mut self, name: &str, writer: &str) {
        let binary = self.deps.binary();
        let io = self.deps.io();
        let math = self.deps.math();
        let slog = self.deps.slog();
        uwrite!(
            self.src,
            r#"func(v float32, w {io}.Writer) (err error) {{
                b := make([]byte, 4)
                {binary}.LittleEndian.PutUint32(b, {math}.Float32bits(v))
                {slog}.Debug("writing f32")
                _, err = w.Write(b)
                return err
            }}({name}, {writer})"#,
        );
    }

    fn print_write_f64(&mut self, name: &str, writer: &str) {
        let binary = self.deps.binary();
        let io = self.deps.io();
        let math = self.deps.math();
        let slog = self.deps.slog();
        uwrite!(
            self.src,
            r#"func(v float64, w {io}.Writer) (err error) {{
                b := make([]byte, 8)
                {binary}.LittleEndian.PutUint64(b, {math}.Float64bits(v))
                {slog}.Debug("writing f64")
                _, err = w.Write(b)
                return err
            }}({name}, {writer})"#,
        );
    }

    fn print_write_char(&mut self, name: &str, writer: &str) {
        let binary = self.deps.binary();
        let io = self.deps.io();
        let slog = self.deps.slog();
        uwrite!(
            self.src,
            r#"func(v rune, w {io}.Writer) (err error) {{
                b := make([]byte, {binary}.MaxVarintLen32)
                i := {binary}.PutUvarint(b, uint64(v))
                {slog}.Debug("writing char")
                _, err = w.Write(b[:i])
                return err
            }}({name}, {writer})"#,
        );
    }

    fn print_write_string(&mut self, name: &str, writer: &str) {
        let binary = self.deps.binary();
        let io = self.deps.io();
        let fmt = self.deps.fmt();
        let math = self.deps.math();
        let slog = self.deps.slog();
        uwrite!(
            self.src,
            r#"func(v string, w {io}.Writer) (err error) {{
                n := len(v)
                if n > {math}.MaxUint32 {{
                    return {fmt}.Errorf("string byte length of %d overflows a 32-bit integer", n)
                }}
                if err = func(v int, w {io}.Writer) error {{
                    b := make([]byte, {binary}.MaxVarintLen32)
                    i := {binary}.PutUvarint(b, uint64(v))
                    {slog}.Debug("writing string byte length", "len", n)
                    _, err = w.Write(b[:i])
                    return err
                }}(n, w); err != nil {{
                    return {fmt}.Errorf("failed to write string byte length of %d: %w", n, err)
                }}
                {slog}.Debug("writing string bytes")
                _, err = w.Write([]byte(v))
                if err != nil {{
                    return {fmt}.Errorf("failed to write string bytes: %w", err)
                }}
                return nil
            }}({name}, {writer})"#,
        );
    }

    fn print_write_list(&mut self, ty: &Type, name: &str, writer: &str) {
        let binary = self.deps.binary();
        let context = self.deps.context();
        let fmt = self.deps.fmt();
        let io = self.deps.io();
        let math = self.deps.math();
        let slog = self.deps.slog();
        let wrpc = self.deps.wrpc();

        self.push_str("func(v ");
        self.print_list(ty);
        uwrite!(
            self.src,
            r#", w interface {{ {io}.ByteWriter; {io}.Writer }}) (write func({context}.Context, {wrpc}.IndexWriter) error, err error) {{
        n := len(v)
        if n > {math}.MaxUint32 {{
            return nil, {fmt}.Errorf("list length of %d overflows a 32-bit integer", n)
        }}
        if err = func(v int, w {io}.Writer) error {{
            b := make([]byte, {binary}.MaxVarintLen32)
            i := {binary}.PutUvarint(b, uint64(v))
            {slog}.Debug("writing list length", "len", n)
            _, err = w.Write(b[:i])
            return err
        }}(n, w); err != nil {{
            return nil, {fmt}.Errorf("failed to write list length of %d: %w", n, err)
        }}
        {slog}.Debug("writing list elements")
        writes := make(map[uint32]func({context}.Context, {wrpc}.IndexWriter) error, n)
        for i, e := range v {{
            write, err := "#
        );
        self.print_write_ty(ty, "e", "w");
        uwrite!(
            self.src,
            r#"
            if err != nil {{
                return nil, {fmt}.Errorf("failed to write list element %d: %w", i, err)
            }}
            if write != nil {{
                writes[uint32(i)] = write
            }}
        }}
        return {wrpc}.JoinWrites(writes), nil
    }}({name}, {writer})"#
        );
    }

    fn print_write_option(&mut self, ty: &Type, name: &str, writer: &str) {
        let context = self.deps.context();
        let fmt = self.deps.fmt();
        let io = self.deps.io();
        let slog = self.deps.slog();
        let wrpc = self.deps.wrpc();

        self.push_str("func(v ");
        self.print_option(ty, true);
        uwrite!(
            self.src,
            r#", w interface {{ {io}.ByteWriter; {io}.Writer }}) (func({context}.Context, {wrpc}.IndexWriter) error, error) {{
        if v == nil {{
            {slog}.Debug("writing `option::none` status byte")
            if err := w.WriteByte(0); err != nil {{
                return nil, {fmt}.Errorf("failed to write `option::none` byte: %w", err)
            }}
            return nil, nil
        }}
        {slog}.Debug("writing `option::some` status byte")
        if err := w.WriteByte(1); err != nil {{
            return nil, {fmt}.Errorf("failed to write `option::some` status byte: %w", err)
        }}
        {slog}.Debug("writing `option::some` payload")
        write, err := "#
        );
        let ptr = self.nillable_ptr(ty, false, true);
        let param = format!("{ptr}v");
        self.print_write_ty(ty, &param, "w");
        uwrite!(
            self.src,
            r#"
        if err != nil {{
            return nil, {fmt}.Errorf("failed to write `option::some` payload: %w", err)
        }}
        return write, nil
    }}({name}, {writer})"#
        );
    }

    fn print_write_result(&mut self, ty: &Result_, name: &str, writer: &str) {
        let context = self.deps.context();
        let errors = self.deps.errors();
        let fmt = self.deps.fmt();
        let io = self.deps.io();
        let slog = self.deps.slog();
        let wrpc = self.deps.wrpc();

        self.push_str("func(v *");
        self.print_result(ty);
        uwriteln!(
            self.src,
            r#", w interface {{ {io}.ByteWriter; {io}.Writer }}) (func({context}.Context, {wrpc}.IndexWriter) error, error) {{
        switch {{
            case v.Ok == nil && v.Err == nil:
                return nil, {errors}.New("both result variants cannot be nil")
            case v.Ok != nil && v.Err != nil:
                return nil, {errors}.New("exactly one result variant must non-nil")"#
        );
        uwriteln!(
            self.src,
            r#"
            case v.Ok != nil:
                {slog}.Debug("writing `result::ok` status byte")
                if err := w.WriteByte(0); err != nil {{
                    return nil, {fmt}.Errorf("failed to write `result::ok` status byte: %w", err)
                }}"#
        );
        if let Some(ref ty) = ty.ok {
            uwrite!(
                self.src,
                r#"{slog}.Debug("writing `result::ok` payload")
                    write, err := "#
            );
            let ptr = self.nillable_ptr(ty, true, true);
            let param = format!("{ptr}v.Ok");
            self.print_write_ty(ty, &param, "w");
            uwriteln!(
                self.src,
                r#"
                    if err != nil {{
                        return nil, {fmt}.Errorf("failed to write `result::ok` payload: %w", err)
                    }}
                    if write != nil {{
                        return write, nil
                    }}"#
            );
        }
        uwriteln!(
            self.src,
            r#"return nil, nil
            default:
                {slog}.Debug("writing `result::err` status byte")
                if err := w.WriteByte(1); err != nil {{
                    return nil, {fmt}.Errorf("failed to write `result::err` status byte: %w", err)
                }}"#
        );
        if let Some(ref ty) = ty.err {
            uwrite!(
                self.src,
                r#"{slog}.Debug("writing `result::err` payload")
                write, err := "#
            );
            let ptr = self.nillable_ptr(ty, true, true);
            let param = format!("{ptr}v.Err");
            self.print_write_ty(ty, &param, "w");
            uwriteln!(
                self.src,
                r#"
                if err != nil {{
                    return nil, {fmt}.Errorf("failed to write `result::err` payload: %w", err)
                }}
                if write != nil {{
                    return write, nil
                }}"#
            );
        }
        uwrite!(
            self.src,
            r#"return nil, nil
        }}
    }}({name}, {writer})"#
        );
    }

    fn print_write_tuple(&mut self, ty: &Tuple, name: &str, writer: &str) {
        match ty.types.as_slice() {
            [] => {
                let context = self.deps.context();
                let wrpc = self.deps.wrpc();
                uwrite!(
                    self.src,
                    "(func({context}.Context, {wrpc}.IndexWriter) error)(nil), error(nil)"
                );
            }
            [ty] => {
                let context = self.deps.context();
                let fmt = self.deps.fmt();
                let io = self.deps.io();
                let slog = self.deps.slog();
                let wrpc = self.deps.wrpc();

                self.push_str("func(v ");
                self.print_opt_ty(ty, true);
                uwrite!(
                    self.src,
                    r#", w interface {{ {io}.ByteWriter; {io}.Writer }}) (func({context}.Context, {wrpc}.IndexWriter) error, error) {{
        {slog}.Debug("writing tuple element 0")
        write, err := "#
                );
                self.print_write_ty(ty, "v", "w");
                uwrite!(
                    self.src,
                    r#"
        if err != nil {{
            return nil, {fmt}.Errorf("failed to write tuple element 0: %w", err)
        }}
        return {wrpc}.IndexWrite(write, 0), nil
    }}({name}, {writer})"#
                );
            }
            _ => {
                let context = self.deps.context();
                let fmt = self.deps.fmt();
                let io = self.deps.io();
                let wrpc = self.deps.wrpc();

                self.push_str("func(v *");
                self.print_tuple(ty, true);
                uwriteln!(
                    self.src,
                    r", w interface {{ {io}.ByteWriter; {io}.Writer }}) (func({context}.Context, {wrpc}.IndexWriter) error, error) {{
        writes := make(map[uint32]func({context}.Context, {wrpc}.IndexWriter) error, {})",
                    ty.types.len(),
                );
                for (i, ty) in ty.types.iter().enumerate() {
                    let slog = self.deps.slog();
                    uwrite!(
                        self.src,
                        r#"{slog}.Debug("writing tuple element {i}")
        write{i}, err := "#
                    );
                    self.print_write_ty(ty, &format!("v.V{i}"), "w");
                    uwriteln!(
                        self.src,
                        r#"
        if err != nil {{
            return nil, {fmt}.Errorf("failed to write tuple element {i}: %w", err)
        }}
        if write{i} != nil {{
                writes[{i}] = write{i}
        }}"#
                    );
                }
                uwrite!(
                    self.src,
                    r#"return {wrpc}.JoinWrites(writes), nil
    }}({name}, {writer})"#
                );
            }
        }
    }

    fn print_write_future(&mut self, ty: &Option<Type>, name: &str, writer: &str) {
        let wrpc = self.deps.wrpc();
        if let Some(ty) = ty {
            uwrite!(self.src, "{wrpc}.WriteFuture({name}, {writer}, ");
            self.print_write_element(ty);
            self.push_str(")");
        } else {
            uwrite!(self.src, "{wrpc}.WriteFutureUnit({name}, {writer})");
        }
    }

    fn print_write_element(&mut self, ty: &Type) {
        let context = self.deps.context();
        let wrpc = self.deps.wrpc();
        self.push_str("func(v ");
        self.print_opt_ty(ty, true);
        uwrite!(
            self.src,
            ", w {wrpc}.ByteWriter) (func({context}.Context, {wrpc}.IndexWriter) error, error) {{\nreturn "
        );
        self.print_write_ty(ty, "v", "w");
        self.push_str("\n}");
    }

    fn print_write_stream(&mut self, element: &Option<Type>, name: &str, writer: &str) {
        let wrpc = self.deps.wrpc();
        match element {
            Some(ty) if is_ty(self.resolve, Type::U8, ty) => {
                uwrite!(self.src, "{wrpc}.WriteByteStream({name}, {writer})");
            }
            Some(ty) => {
                uwrite!(self.src, "{wrpc}.WriteStream({name}, {writer}, ");
                self.print_write_element(ty);
                self.push_str(")");
            }
            None => panic!("streams with no element types are not supported"),
        }
    }

    fn print_write_ty(&mut self, ty: &Type, name: &str, writer: &str) {
        match ty {
            Type::Id(t) => self.print_write_tyid(*t, name, writer),
            Type::Bool => {
                self.print_nil_write();
                self.print_write_bool(name, writer);
            }
            Type::U8 => {
                self.print_nil_write();
                self.print_write_u8(name, writer);
            }
            Type::U16 => {
                self.print_nil_write();
                self.print_write_u16(name, writer);
            }
            Type::U32 => {
                self.print_nil_write();
                self.print_write_u32(name, writer);
            }
            Type::U64 => {
                self.print_nil_write();
                self.print_write_u64(name, writer);
            }
            Type::S8 => {
                self.print_nil_write();
                self.print_write_s8(name, writer);
            }
            Type::S16 => {
                self.print_nil_write();
                self.print_write_s16(name, writer);
            }
            Type::S32 => {
                self.print_nil_write();
                self.print_write_s32(name, writer);
            }
            Type::S64 => {
                self.print_nil_write();
                self.print_write_s64(name, writer);
            }
            Type::F32 => {
                self.print_nil_write();
                self.print_write_f32(name, writer);
            }
            Type::F64 => {
                self.print_nil_write();
                self.print_write_f64(name, writer);
            }
            Type::Char => {
                self.print_nil_write();
                self.print_write_char(name, writer);
            }
            Type::String => {
                self.print_nil_write();
                self.print_write_string(name, writer);
            }
            Type::ErrorContext => panic!("error-context is not supported"),
        }
    }

    fn print_write_tyid(&mut self, id: TypeId, name: &str, writer: &str) {
        let ty = &self.resolve.types[id];
        match &ty.kind {
            TypeDefKind::Handle(Handle::Own(_id)) => {
                self.print_nil_write();
                self.print_write_string(&format!("string({name})"), writer);
            }
            TypeDefKind::Handle(Handle::Borrow(_id)) => {
                self.print_nil_write();
                self.print_write_string(&format!("string({name})"), writer);
            }
            TypeDefKind::Tuple(ty) => self.print_write_tuple(ty, name, writer),
            TypeDefKind::Option(ty) => self.print_write_option(ty, name, writer),
            TypeDefKind::Result(ty) => self.print_write_result(ty, name, writer),
            TypeDefKind::List(ty) => self.print_write_list(ty, name, writer),
            TypeDefKind::Future(ty) => self.print_write_future(ty, name, writer),
            TypeDefKind::Stream(ty) => self.print_write_stream(ty, name, writer),
            TypeDefKind::Type(ty) => self.print_write_ty(ty, name, writer),
            _ => {
                if ty.name.is_some() {
                    uwrite!(self.src, "({name}).WriteToIndex({writer})");
                    return;
                }
                match &ty.kind {
                    TypeDefKind::Record(_) => {
                        panic!("unsupported anonymous type reference: record")
                    }
                    TypeDefKind::Resource => {
                        panic!("unsupported anonymous type reference: resource")
                    }
                    TypeDefKind::Flags(_) => panic!("unsupported anonymous type reference: flags"),
                    TypeDefKind::Variant(_) => panic!("unsupported anonymous variant"),
                    TypeDefKind::Enum(_) => panic!("unsupported anonymous type reference: enum"),
                    _ => unreachable!(),
                }
            }
        }
    }

    fn print_read_discriminant(&mut self, repr: Int, reader: &str) {
        match repr {
            Int::U8 => {
                uwrite!(
                    self.src,
                    r#"func(r {io}.ByteReader) (uint8, error) {{
    var x uint8
    var s uint
    for i := 0; i < 2; i++ {{
        {slog}.Debug("reading u8 discriminant byte", "i", i)
        b, err := r.ReadByte()
        if err != nil {{
            if i > 0 && err == {io}.EOF {{
                err = {io}.ErrUnexpectedEOF
            }}
            return x, {fmt}.Errorf("failed to read u8 discriminant byte: %w", err)
        }}
        if s == 7 && b > 0x01 {{
            return x, {errors}.New("discriminant overflows an 8-bit integer")
        }}
        if b < 0x80 {{
            return x | uint8(b)<<s, nil
        }}
        x |= uint8(b&0x7f) << s
        s += 7
    }}
    return x, {errors}.New("discriminant overflows an 8-bit integer")
}}({reader})"#,
                    errors = self.deps.errors(),
                    fmt = self.deps.fmt(),
                    io = self.deps.io(),
                    slog = self.deps.slog(),
                );
            }
            Int::U16 => self.print_read_u16(reader),
            Int::U32 => self.print_read_u32(reader),
            Int::U64 => self.print_read_u64(reader),
        }
    }

    fn print_write_discriminant(&mut self, repr: Int, name: &str, writer: &str) {
        match repr {
            Int::U8 => uwrite!(
                self.src,
                r#"func(v uint8, w {io}.Writer) error {{
                b := make([]byte, 2)
                i := {binary}.PutUvarint(b, uint64(v))
                {slog}.Debug("writing u8 discriminant")
                _, err := w.Write(b[:i])
                return err
            }}(uint8({name}), {writer})"#,
                binary = self.deps.binary(),
                io = self.deps.io(),
                slog = self.deps.slog(),
            ),
            Int::U16 => uwrite!(
                self.src,
                r#"func(v uint16, w {io}.Writer) error {{
                b := make([]byte, {binary}.MaxVarintLen16)
                i := {binary}.PutUvarint(b, uint64(v))
                {slog}.Debug("writing u16 discriminant")
                _, err := w.Write(b[:i])
                return err
            }}(uint16({name}), {writer})"#,
                binary = self.deps.binary(),
                io = self.deps.io(),
                slog = self.deps.slog(),
            ),
            Int::U32 => uwrite!(
                self.src,
                r#"func(v uint32, w {io}.Writer) error {{
                b := make([]byte, {binary}.MaxVarintLen32)
                i := {binary}.PutUvarint(b, uint64(v))
                {slog}.Debug("writing u32 discriminant")
                _, err := w.Write(b[:i])
                return err
            }}(uint32({name}), {writer})"#,
                binary = self.deps.binary(),
                io = self.deps.io(),
                slog = self.deps.slog(),
            ),
            Int::U64 => uwrite!(
                self.src,
                r#"func(v uint64, w {io}.Writer) error {{
                b := make([]byte, {binary}.MaxVarintLen64)
                i := {binary}.PutUvarint(b, uint64(v))
                {slog}.Debug("writing u64 discriminant")
                _, err := w.Write(b[:i])
                return err
            }}(uint64({name}), {writer})"#,
                binary = self.deps.binary(),
                io = self.deps.io(),
                slog = self.deps.slog(),
            ),
        }
    }

    pub(super) fn generate_exports<'a>(
        &mut self,
        identifier: Identifier<'a>,
        funcs: impl Iterator<Item = &'a Function>,
    ) -> bool {
        let mut methods = vec![];
        let mut funcs_to_export = vec![];

        for func in funcs {
            if self.r#gen.skip.contains(&func.name) {
                continue;
            }

            funcs_to_export.push(func);
            let prev = mem::take(&mut self.src);
            self.print_docs_and_params(func);
            self.src.push_str(" (");
            for ty in func
                .result
                .iter()
                .flat_map(|ty| flatten_ty(self.resolve, ty))
            {
                self.print_opt_ty(&ty, true);
                self.src.push_str(", ");
            }
            self.push_str("error)\n");
            let trait_method = mem::replace(&mut self.src, prev);
            methods.push(trait_method);
        }

        if methods.is_empty() {
            return false;
        }

        uwriteln!(self.src, "type Handler interface {{");
        for method in &methods {
            self.src.push_str(method);
        }
        uwriteln!(
            self.src,
            "
}}

func ServeInterface(s {wrpc}.Server, h Handler) (stop func() error, err error) {{
    stops := make([]func() error, 0, {})
    stop = func() error {{
        for _, stop := range stops {{
            if err := stop(); err != nil {{
                return err
            }}
        }}
        return nil
    }}",
            funcs_to_export.len(),
            wrpc = self.deps.wrpc(),
        );
        let instance = match identifier {
            Identifier::Interface(id, name) => {
                let interface = &self.resolve.interfaces[id];
                let name = match name {
                    WorldKey::Name(s) => s.to_string(),
                    WorldKey::Interface(..) => interface
                        .name
                        .as_ref()
                        .expect("interface name missing")
                        .to_string(),
                };
                if let Some(package) = interface.package {
                    self.resolve.id_of_name(package, &name)
                } else {
                    name
                }
            }
            Identifier::World => String::new(),
        };
        for (i, func) in funcs_to_export.iter().enumerate() {
            let name = rpc_func_name(func);
            let fqn = if instance.is_empty() {
                name.to_string()
            } else {
                format!("{instance}.{name}")
            };

            let bytes = self.deps.bytes();
            let context = self.deps.context();
            let fmt = self.deps.fmt();
            let slog = self.deps.slog();
            let wrpc = self.deps.wrpc();
            uwrite!(
                self.src,
                r#"
    stop{i}, err := s.Serve("{instance}", "{name}", func(ctx {context}.Context, w {wrpc}.IndexWriteCloser, r {wrpc}.IndexReadCloser) {{
        defer func() {{
            if err := w.Close(); err != nil {{
                {slog}.DebugContext(ctx, "failed to close writer", "instance", "{instance}", "name", "{name}", "err", err)
            }}
        }}()"#,
            );
            let async_params = func.params.iter().any(|Param { ty, .. }| self.is_async(ty));
            let abort = |indent: usize| {
                if async_params {
                    format!("\n{:indent$}cancel()", "")
                } else {
                    String::new()
                }
            };
            if async_params {
                let sync = self.deps.sync();
                uwrite!(
                    self.src,
                    r#"
        deferredCtx, cancel := {context}.WithCancel(ctx)
        var wg {sync}.WaitGroup
        defer func() {{
            go func() {{
                wg.Wait()
                cancel()
            }}()
        }}()"#,
                );
            }
            for (i, Param { ty, .. }) in func.params.iter().enumerate() {
                let d = if self.is_async(ty) {
                    format!("d{i}, ")
                } else {
                    String::new()
                };
                uwrite!(
                    self.src,
                    r#"
        {slog}.DebugContext(ctx, "reading parameter", "i", {i})
        p{i}, {d}err := "#
                );
                self.print_read_ty(ty, "r", &format!("[]uint32{{ {i} }}"));
                self.push_str("\n");
                uwrite!(
                    self.src,
                    r#"
        if err != nil {{
            {slog}.WarnContext(ctx, "failed to read parameter", "i", {i}, "instance", "{instance}", "name", "{name}", "err", err)
            if err := r.Close(); err != nil {{
                {slog}.ErrorContext(ctx, "failed to close reader", "instance", "{instance}", "name", "{name}", "err", err)
            }}
            return
        }}"#,
                );
            }
            for (i, Param { ty, .. }) in func.params.iter().enumerate() {
                if self.is_async(ty) {
                    uwrite!(
                        self.src,
                        r#"
        if d{i} != nil {{
            wg.Add(1)
            go func() {{
                defer wg.Done()
                if err := d{i}(deferredCtx); err != nil {{
                    {slog}.WarnContext(ctx, "failed to read deferred parameter", "i", {i}, "instance", "{instance}", "name", "{name}", "err", err)
                }}
            }}()
        }}"#,
                    );
                }
            }
            uwriteln!(
                self.src,
                r#"
        {slog}.DebugContext(ctx, "calling `{fqn}` handler")"#,
            );
            let results: Box<[Type]> = func
                .result
                .iter()
                .flat_map(|ty| flatten_ty(self.resolve, ty))
                .collect();
            for (i, _) in results.iter().enumerate() {
                uwrite!(self.src, "r{i}, ");
            }
            self.push_str("err ");
            if !results.is_empty() {
                self.push_str(":");
            }
            self.push_str("= h.");
            self.push_str(&go_func_name(func));
            self.push_str("(ctx");
            for (i, _) in func.params.iter().enumerate() {
                uwrite!(self.src, ", p{i}");
            }
            let abort12 = abort(12);
            uwriteln!(
                self.src,
                r#")
        if cErr := r.Close(); cErr != nil {{
            {slog}.ErrorContext(ctx, "failed to close reader", "instance", "{instance}", "name", "{name}", "err", cErr)
        }}
        if err != nil {{
            {slog}.WarnContext(ctx, "failed to handle invocation", "instance", "{instance}", "name", "{name}", "err", err){abort12}
            return
        }}

        var buf {bytes}.Buffer
        writes := make(map[uint32]func({context}.Context, {wrpc}.IndexWriter) error, {})"#,
                results.len()
            );
            for (i, ty) in results.iter().enumerate() {
                uwrite!(
                    self.src,
                    r#"
        write{i}, err := "#
                );
                self.print_write_ty(ty, &format!("r{i}"), "&buf");
                uwrite!(
                    self.src,
                    r#"
        if err != nil {{
            {slog}.WarnContext(ctx, "failed to write result value", "i", {i}, "instance", "{instance}", "name", "{name}", "err", err){abort12}
            return
        }}
        if write{i} != nil {{
            writes[{i}] = write{i}
        }}"#,
                );
            }
            uwrite!(
                self.src,
                r#"
        {slog}.DebugContext(ctx, "transmitting `{fqn}` result")
        _, err = w.Write(buf.Bytes())
        if err != nil {{
            {slog}.WarnContext(ctx, "failed to write result", "instance", "{instance}", "name", "{name}", "err", err){abort12}
            return
        }}
        if len(writes) > 0 {{
            for index, write := range writes {{
                _ = write
                switch index {{"#
            );

            let mut idx = 0usize;
            for (i, ty) in func.result.iter().enumerate() {
                for (j, _) in flatten_ty(self.resolve, ty).enumerate() {
                    uwrite!(
                        self.src,
                        r#"
                    case {idx}:
                        w, err := w.Index({i}"#,
                    );
                    if is_tuple(self.resolve, ty) {
                        uwrite!(self.src, ", {j}");
                    }
                    let abort28 = abort(28);
                    uwrite!(
                        self.src,
                        r#")
                        if err != nil {{
                            {slog}.ErrorContext(ctx, "failed to index result writer", "instance", "{instance}", "name", "{name}", "err", err){abort28}
                            return
                        }}
                        write := write"#,
                    );
                    if async_params {
                        self.push_str(
                            r#"
                        wg.Add(1)"#,
                        );
                    }
                    self.push_str(
                        r#"
                        go func() {"#,
                    );
                    if async_params {
                        self.push_str(
                            r#"
                            defer wg.Done()"#,
                        );
                    }
                    let abort32 = abort(32);
                    uwrite!(
                        self.src,
                        r#"
                            if err := write(ctx, w); err != nil {{
                                {slog}.WarnContext(ctx, "failed to write nested result value", "instance", "{instance}", "name", "{name}", "err", err){abort32}
                            }}
                            if err := w.Close(); err != nil {{
                                {slog}.DebugContext(ctx, "failed to close nested result writer", "instance", "{instance}", "name", "{name}", "err", err)
                            }}
                        }}()"#,
                    );
                    idx = idx.saturating_add(1);
                }
            }

            uwrite!(
                self.src,
                r#"
                }}
            }}
        }}
    }}, "#,
            );
            for (i, Param { ty, .. }) in func.params.iter().enumerate() {
                let (nested, fut) = async_paths_ty(self.resolve, ty);
                for path in nested {
                    self.push_str(wrpc);
                    self.push_str(".NewSubscribePath().Index(");
                    uwrite!(self.src, "{i})");
                    for p in path {
                        if let Some(p) = p {
                            uwrite!(self.src, ".Index({p})");
                        } else {
                            self.push_str(".Wildcard()");
                        }
                    }
                    self.push_str(", ");
                }
                if fut {
                    uwrite!(self.src, "{wrpc}.NewSubscribePath().Index({i}), ");
                }
            }
            uwriteln!(
                self.src,
                r#")
             if err != nil {{
                 return nil, {fmt}.Errorf("failed to serve `{fqn}`: %w", err)
             }}
             stops = append(stops, stop{i})"#,
            );
        }
        self.push_str("return stop, nil\n");
        self.push_str("}\n");
        true
    }

    pub fn generate_imports<'a>(
        &mut self,
        _identifier: Identifier<'a>,
        instance: &str,
        funcs: impl Iterator<Item = &'a Function>,
    ) {
        for func in funcs {
            if self.r#gen.skip.contains(&func.name) {
                continue;
            }

            let context = self.deps.context();
            let fmt = self.deps.fmt();
            let wrpc = self.deps.wrpc();

            self.print_docs_and_params(func);

            self.src.push_str(" (");
            let results: Box<[Type]> = func
                .result
                .iter()
                .flat_map(|ty| flatten_ty(self.resolve, ty))
                .collect();
            for (i, ty) in results.iter().enumerate() {
                uwrite!(self.src, "r{i}__ ");
                self.print_opt_ty(ty, true);
                self.src.push_str(", ");
            }

            let async_params = func.params.iter().any(|param| self.is_async(&param.ty));
            let async_results = results.iter().filter(|ty| self.is_async(ty)).count();
            if async_params || async_results > 0 {
                self.push_str("errs__ <-chan error, ");
            }
            self.push_str("err__ error) {");
            if !func.params.is_empty() {
                let bytes = self.deps.bytes();
                uwrite!(
                    self.src,
                    r"
    var buf__ {bytes}.Buffer",
                );
                if async_params {
                    uwrite!(
                        self.src,
                        r"
    var writeCount__ uint32"
                    );
                }
                for (i, Param { name, ty, .. }) in func.params.iter().enumerate() {
                    uwrite!(
                        self.src,
                        r"
    write{i}__, err__ :="
                    );
                    self.print_write_ty(ty, &to_go_ident(name), "&buf__");
                    uwrite!(
                        self.src,
                        r#"
    if err__ != nil {{
        err__ = {fmt}.Errorf("failed to write `{name}` parameter: %w", err__)
        return
    }}"#,
                    );
                    if async_params {
                        uwrite!(
                            self.src,
                            r"
    if write{i}__ != nil {{ 
        writeCount__++
    }}"
                        );
                    }
                }
                if async_params {
                    uwrite!(
                        self.src,
                        r"
    writes__ := make(map[uint32]func({context}.Context, {wrpc}.IndexWriter) error, uint(writeCount__))",
                    );
                }
                for (i, Param { name, .. }) in func.params.iter().enumerate() {
                    uwrite!(
                        self.src,
                        r"
    if write{i}__ != nil {{"
                    );
                    if async_params {
                        uwrite!(
                            self.src,
                            r"
        writes__[{i}] = write{i}__",
                        );
                    } else {
                        uwrite!(
                            self.src,
                            r#"
        err__ = {errors}.New("unexpected deferred write for synchronous `{name}` parameter")
        return"#,
                            errors = self.deps.errors(),
                        );
                    }
                    uwrite!(
                        self.src,
                        r#"
    }}"#,
                    );
                }
            }
            uwrite!(
                self.src,
                r#"
    var w__ {wrpc}.IndexWriteCloser
    var r__ {wrpc}.IndexReadCloser
    w__, r__, err__ = wrpc__.Invoke(ctx__, "{instance}", ""#
            );
            self.src.push_str(rpc_func_name(func));
            self.src.push_str("\", ");
            if !func.params.is_empty() {
                self.src.push_str("buf__.Bytes()");
            } else {
                self.src.push_str("nil");
            }
            self.src.push_str(",\n");
            for (i, ty) in func.result.iter().enumerate() {
                let (nested, fut) = async_paths_ty(self.resolve, ty);
                for path in nested {
                    uwrite!(self.src, "{wrpc}.NewSubscribePath().Index({i})");
                    for p in path {
                        if let Some(p) = p {
                            uwrite!(self.src, ".Index({p})");
                        } else {
                            self.push_str(".Wildcard()");
                        }
                    }
                    self.push_str(", ");
                }
                if fut {
                    uwrite!(self.src, "{wrpc}.NewSubscribePath().Index({i}), ");
                }
            }
            let slog = self.deps.slog();
            uwrite!(
                self.src,
                r#"
    )
    if err__ != nil {{
        err__ = {fmt}.Errorf("failed to invoke `{name}`: %w", err__)
        return
    }}
    defer func() {{
        if err := r__.Close(); err != nil {{
            {slog}.ErrorContext(ctx__, "failed to close reader", "instance", "{instance}", "name", "{name}", "err", err)
        }}
    }}()"#,
                name = func.name,
            );
            if async_params || async_results > 0 {
                let sync = self.deps.sync();
                let count = if async_params {
                    format!("uint(writeCount__) + {async_results}")
                } else {
                    async_results.to_string()
                };
                uwrite!(
                    self.src,
                    r#"
    errCh__ := make(chan error, {count})
    errs__ = errCh__
    var wg__ {sync}.WaitGroup
    defer func() {{
        go func() {{
            wg__.Wait()
            close(errCh__)
        }}()
    }}()"#
                );
            }
            if async_params {
                uwrite!(
                    self.src,
                    r#"
    if writeCount__ > 0 {{
        for index, write := range writes__ {{
            w, err := w__.Index(index)
            if err != nil {{
                if cErr := w__.Close(); cErr != nil {{
                    {slog}.DebugContext(ctx__, "failed to close outgoing stream", "instance", "{instance}", "name", "{0}", "err", cErr)
                }}
                err__ = {fmt}.Errorf("failed to index param writer at index `%v`: %w", index, err)
                return
            }}
            wg__.Add(1)
            write := write
            go func() {{
                defer wg__.Done()
                if err := write(ctx__, w); err != nil {{
                    errCh__ <- err
                }}
                if err := w.Close(); err != nil {{
                    {slog}.DebugContext(ctx__, "failed to close nested param writer", "instance", "{instance}", "name", "{0}", "err", err)
                }}
            }}()
        }}
    }}"#,
                    func.name,
                );
            }
            uwrite!(
                self.src,
                r#"
    if cErr__ := w__.Close(); cErr__ != nil {{
        {slog}.DebugContext(ctx__, "failed to close outgoing stream", "instance", "{instance}", "name", "{}", "err", cErr__)
    }}"#,
                func.name,
            );

            let mut idx = 0usize;
            let mut deferreds = Vec::default();
            for (i, rty) in func.result.iter().enumerate() {
                for (j, ty) in flatten_ty(self.resolve, rty).enumerate() {
                    let deferred = self.is_async(&ty);
                    if deferred {
                        let context = self.deps.context();
                        uwrite!(
                            self.src,
                            "
    var d{idx}__ func({context}.Context) error
    r{idx}__, d{idx}__, err__ = "
                        );
                    } else {
                        uwrite!(
                            self.src,
                            "
    r{idx}__, err__ = "
                        );
                    }
                    let path = if is_tuple(self.resolve, rty) {
                        format!("[]uint32{{ {i}, {j} }}")
                    } else {
                        format!("[]uint32{{ {idx} }}")
                    };
                    self.print_read_ty(&ty, "r__", &path);
                    uwrite!(
                        self.src,
                        r#"
    if err__ != nil {{
        err__ = {fmt}.Errorf("failed to read result {idx}: %w", err__)
        return 
    }}"#,
                    );
                    if deferred {
                        deferreds.push(idx);
                    }
                    idx = idx.saturating_add(1);
                }
            }
            for idx in deferreds {
                uwrite!(
                    self.src,
                    r#"
    if d{idx}__ != nil {{
        wg__.Add(1)
        go func() {{
            defer wg__.Done()
            if err := d{idx}__(ctx__); err != nil {{
                errCh__ <- err
            }}
        }}()
    }}"#,
                );
            }
            if func.result.is_none() {
                // A function with no results still completes with an (empty)
                // result transmission; await it so the call is synchronous and
                // the invocation's server-side effects are observable on return.
                let io = self.deps.io();
                let fmt = self.deps.fmt();
                uwrite!(
                    self.src,
                    r#"
    if _, err__ = {io}.ReadAll(r__); err__ != nil {{
        err__ = {fmt}.Errorf("failed to await `{name}` completion: %w", err__)
        return
    }}
    err__ = nil"#,
                    name = func.name,
                );
            }
            uwriteln!(
                self.src,
                r#"
    return
}}"#,
            );
        }
    }

    pub fn finish(&mut self) -> String {
        mem::take(&mut self.src).into()
    }

    pub fn start_append_submodule(&mut self, name: &WorldKey) -> (String, Vec<String>) {
        let snake = match name {
            WorldKey::Name(name) => to_package_ident(name),
            WorldKey::Interface(id) => {
                to_package_ident(self.resolve.interfaces[*id].name.as_ref().unwrap())
            }
        };
        let module_path = crate::compute_module_path(name, self.resolve, !self.in_import);
        (snake, module_path)
    }

    pub fn finish_append_submodule(mut self, snake: &str, module_path: Vec<String>) {
        let module = self.finish();
        let module = format!(
            r#"package {snake}

{}

{module}"#,
            self.deps,
        );
        let map = if self.in_import {
            &mut self.r#gen.import_modules
        } else {
            &mut self.r#gen.export_modules
        };
        map.push((module, module_path));
    }

    fn godoc(&mut self, docs: &Docs) {
        let docs = match &docs.contents {
            Some(docs) => docs,
            None => return,
        };
        for line in docs.trim().lines() {
            self.push_str("//");
            if !line.is_empty() {
                self.push_str(" ");
                self.push_str(line);
            }
            self.push_str("\n");
        }
    }

    fn godoc_params(&mut self, docs: &[Param], header: &str) {
        let _ = (docs, header);
        // let docs = docs
        //     .iter()
        //     .filter(|param| param.docs.trim().len() > 0)
        //     .collect::<Vec<_>>();
        // if docs.len() == 0 {
        //     return;
        // }

        // self.push_str("///\n");
        // self.push_str("/// ## ");
        // self.push_str(header);
        // self.push_str("\n");
        // self.push_str("///\n");

        // for param in docs {
        //     for (i, line) in param.docs.lines().enumerate() {
        //         self.push_str("/// ");
        //         // Currently wasi only has at most one return value, so there's no
        //         // need to indent it or name it.
        //         if header != "Return" {
        //             if i == 0 {
        //                 self.push_str("* `");
        //                 self.push_str(to_go_ident(param.name.as_str()));
        //                 self.push_str("` - ");
        //             } else {
        //                 self.push_str("  ");
        //             }
        //         }
        //         self.push_str(line);
        //         self.push_str("\n");
        //     }
        // }
    }

    fn print_docs_and_params(&mut self, func: &Function) {
        self.godoc(&func.docs);
        self.godoc_params(&func.params, "Parameters");
        // TODO: re-add this when docs are back
        // self.godoc_params(&func.results, "Return");

        if self.in_import {
            self.push_str("func ");
        }
        if self.in_import && matches!(func.kind, FunctionKind::Constructor(..)) {
            self.push_str("New");
        }
        self.push_str(&go_func_name(func));
        let context = self.deps.context();
        uwrite!(self.src, "(ctx__ {context}.Context, ");
        if self.in_import {
            let wrpc = self.deps.wrpc();
            uwrite!(self.src, "wrpc__ {wrpc}.Invoker, ");
        }
        for Param {
            name, ty: param, ..
        } in &func.params
        {
            self.push_str(&to_go_ident(name));
            self.push_str(" ");
            self.print_opt_ty(param, true);
            self.push_str(",");
        }
        self.push_str(")");
    }

    fn print_ty(&mut self, ty: &Type, decl: bool) {
        match ty {
            Type::Id(t) => self.print_tyid(*t, decl),
            Type::Bool => self.push_str("bool"),
            Type::U8 => self.push_str("uint8"),
            Type::U16 => self.push_str("uint16"),
            Type::U32 => self.push_str("uint32"),
            Type::U64 => self.push_str("uint64"),
            Type::S8 => self.push_str("int8"),
            Type::S16 => self.push_str("int16"),
            Type::S32 => self.push_str("int32"),
            Type::S64 => self.push_str("int64"),
            Type::F32 => self.push_str("float32"),
            Type::F64 => self.push_str("float64"),
            Type::Char => self.push_str("rune"),
            Type::String => self.push_str("string"),
            Type::ErrorContext => self.push_str("struct{}"),
        }
    }

    fn print_optional_ty(&mut self, ty: Option<&Type>) {
        match ty {
            Some(ty) => self.print_ty(ty, true),
            None => self.push_str("struct{}"),
        }
    }

    fn type_path_with_name(&mut self, id: TypeId, name: String) -> String {
        if let TypeOwner::Interface(id) = self.resolve.types[id].owner
            && let Some(path) = self.path_to_interface(id)
        {
            return format!("{path}.{name}");
        }
        name
    }

    fn print_nil_write(&mut self) {
        let context = self.deps.context();
        let wrpc = self.deps.wrpc();
        uwrite!(
            self.src,
            "(func({context}.Context, {wrpc}.IndexWriter) error)(nil), "
        );
    }

    fn print_deferred_ret(&mut self, deferred: bool) {
        if deferred {
            let context = self.deps.context();
            uwrite!(self.src, ", func({context}.Context) error");
        }
    }

    fn is_async(&self, ty: &Type) -> bool {
        let (paths, fut) = async_paths_ty(self.resolve, ty);
        fut || !paths.is_empty()
    }

    fn nillable_ptr(&self, ty: &Type, result: bool, decl: bool) -> &'static str {
        if let Type::Id(id) = ty {
            match &self.resolve.types[*id].kind {
                TypeDefKind::Option(..)
                | TypeDefKind::Enum(..)
                | TypeDefKind::Resource
                | TypeDefKind::Handle(..) => {}
                TypeDefKind::List(..) | TypeDefKind::Future(..) | TypeDefKind::Stream(..)
                    if result => {}
                TypeDefKind::Tuple(Tuple { types }) if types.len() == 1 => {
                    return self.nillable_ptr(&types[0], result, decl);
                }
                TypeDefKind::Type(ty) => return self.nillable_ptr(ty, result, decl),
                _ => return "",
            }
        }
        if decl { "*" } else { "&" }
    }

    fn print_nillable_ptr(&mut self, ty: &Type, result: bool, decl: bool) {
        let ptr = self.nillable_ptr(ty, result, decl);
        if !ptr.is_empty() {
            self.push_str(ptr);
        }
    }

    fn print_opt_ptr(&mut self, ty: &Type, decl: bool) {
        if let Type::Id(id) = ty {
            let ty = &self.resolve.types[*id];
            match &ty.kind {
                TypeDefKind::Record(..)
                | TypeDefKind::Flags(..)
                | TypeDefKind::Variant(..)
                | TypeDefKind::Result(..) => {
                    if decl {
                        self.push_str("*");
                    } else {
                        self.push_str("&");
                    }
                }
                TypeDefKind::Tuple(ty) if ty.types.len() == 1 => {
                    self.print_opt_ptr(&ty.types[0], decl);
                }
                TypeDefKind::Tuple(ty) if ty.types.len() >= 2 => {
                    if decl {
                        self.push_str("*");
                    } else {
                        self.push_str("&");
                    }
                }
                TypeDefKind::Type(ty) => self.print_opt_ptr(ty, decl),
                _ => {}
            }
        }
    }

    fn print_opt_ty(&mut self, ty: &Type, decl: bool) {
        match ty {
            Type::Id(id) => {
                let ty = &self.resolve.types[*id];
                match &ty.kind {
                    TypeDefKind::Handle(..) => self.print_tyid(*id, decl),
                    TypeDefKind::Tuple(ty) if ty.types.len() < 2 => self.print_tuple(ty, decl),
                    TypeDefKind::Enum(..) => {
                        let name = ty.name.as_ref().expect("enum missing a name");
                        let name = self.type_path_with_name(*id, to_upper_camel_case(name));
                        self.push_str(&name);
                    }
                    TypeDefKind::Option(ty) => self.print_option(ty, decl),
                    TypeDefKind::List(ty) => self.print_list(ty),
                    TypeDefKind::Future(ty) => self.print_future(ty),
                    TypeDefKind::Stream(ty) => self.print_stream(ty),
                    TypeDefKind::Type(ty) => self.print_opt_ty(ty, decl),
                    _ => {
                        if decl {
                            self.push_str("*");
                        } else {
                            self.push_str("&");
                        }
                        self.print_tyid(*id, true);
                    }
                }
            }
            Type::Bool => self.push_str("bool"),
            Type::U8 => self.push_str("uint8"),
            Type::U16 => self.push_str("uint16"),
            Type::U32 => self.push_str("uint32"),
            Type::U64 => self.push_str("uint64"),
            Type::S8 => self.push_str("int8"),
            Type::S16 => self.push_str("int16"),
            Type::S32 => self.push_str("int32"),
            Type::S64 => self.push_str("int64"),
            Type::F32 => self.push_str("float32"),
            Type::F64 => self.push_str("float64"),
            Type::Char => self.push_str("rune"),
            Type::String => self.push_str("string"),
            Type::ErrorContext => self.push_str("struct{}"),
        }
    }

    fn print_option(&mut self, ty: &Type, decl: bool) {
        let mut inner = ty;
        while let Type::Id(id) = inner
            && let TypeDefKind::Type(ty) = &self.resolve.types[*id].kind
        {
            inner = ty;
        }
        if let Type::Id(id) = inner
            && let TypeDefKind::List(..) | TypeDefKind::Future(..) | TypeDefKind::Stream(..) =
                self.resolve.types[*id].kind
        {
            // Go slices and channels are nillable
            self.print_ty(ty, true);
            return;
        }
        if decl {
            self.push_str("*");
        } else {
            self.push_str("&");
        }
        self.print_ty(ty, true);
    }

    fn print_result(&mut self, ty: &Result_) {
        let wrpc = self.deps.wrpc();
        self.push_str(wrpc);
        self.push_str(".Result[");
        self.print_optional_ty(ty.ok.as_ref());
        self.push_str(",");
        self.print_optional_ty(ty.err.as_ref());
        self.push_str("]");
    }

    fn print_tuple(&mut self, Tuple { types }: &Tuple, decl: bool) {
        match types.as_slice() {
            [] => self.push_str("struct{}"),
            [ty] => self.print_opt_ty(ty, decl),
            _ => {
                let wrpc = self.deps.wrpc();
                self.push_str(wrpc);
                self.push_str(".Tuple");
                uwrite!(self.src, "{}[", types.len());
                for ty in types {
                    self.print_opt_ty(ty, true);
                    self.push_str(",");
                }
                self.push_str("]");
            }
        }
    }

    fn print_list(&mut self, ty: &Type) {
        self.push_str("[]");
        self.print_opt_ty(ty, true);
    }

    fn print_future(&mut self, ty: &Option<Type>) {
        self.push_str("<-chan ");
        if let Some(ty) = ty {
            self.print_opt_ty(ty, true);
        } else {
            self.push_str("struct{}");
        }
    }

    fn print_stream(&mut self, element: &Option<Type>) {
        let Some(ty) = element else {
            panic!("streams with no element types are not supported")
        };
        self.push_str("<-chan ");
        self.print_list(ty);
    }

    fn print_own(&mut self, id: TypeId) {
        let wrpc = self.deps.wrpc();
        self.push_str(wrpc);
        self.push_str(".Own[");
        self.print_tyid(id, true);
        self.push_str("]");
    }

    fn print_borrow(&mut self, id: TypeId) {
        let wrpc = self.deps.wrpc();
        self.push_str(wrpc);
        self.push_str(".Borrow[");
        self.print_tyid(id, true);
        self.push_str("]");
    }

    fn print_tyid(&mut self, id: TypeId, decl: bool) {
        let ty = &self.resolve.types[id];
        if let Some(name) = &ty.name {
            let name = self.type_path_with_name(id, to_upper_camel_case(name));
            self.push_str(&name);
            return;
        }
        match &ty.kind {
            TypeDefKind::List(ty) => self.print_list(ty),
            TypeDefKind::FixedLengthList(..) => {
                panic!("unsupported anonymous type reference: fixed size list")
            }
            TypeDefKind::Map(..) => panic!("unsupported anonymous type reference: map"),
            TypeDefKind::Option(ty) => self.print_option(ty, decl),
            TypeDefKind::Result(ty) => self.print_result(ty),
            TypeDefKind::Variant(_) => panic!("unsupported anonymous variant"),
            TypeDefKind::Tuple(ty) => self.print_tuple(ty, decl),
            TypeDefKind::Resource => panic!("unsupported anonymous type reference: resource"),
            TypeDefKind::Record(_) => panic!("unsupported anonymous type reference: record"),
            TypeDefKind::Flags(_) => panic!("unsupported anonymous type reference: flags"),
            TypeDefKind::Enum(_) => panic!("unsupported anonymous type reference: enum"),
            TypeDefKind::Future(ty) => self.print_future(ty),
            TypeDefKind::Stream(ty) => self.print_stream(ty),
            TypeDefKind::Handle(Handle::Own(id)) => self.print_own(*id),
            TypeDefKind::Handle(Handle::Borrow(id)) => self.print_borrow(*id),
            TypeDefKind::Type(t) => self.print_ty(t, decl),
            TypeDefKind::Unknown => unreachable!(),
        }
    }

    fn int_repr(&mut self, repr: Int) {
        match repr {
            Int::U8 => self.push_str("uint8"),
            Int::U16 => self.push_str("uint16"),
            Int::U32 => self.push_str("uint32"),
            Int::U64 => self.push_str("uint64"),
        }
    }

    fn name_of(&self, ty: TypeId) -> Option<String> {
        let info = self.info(ty);

        // If this type isn't actually used, no need to generate it.
        (info.owned || info.borrowed)
            .then(|| to_upper_camel_case(self.resolve.types[ty].name.as_ref().unwrap()))
    }

    fn path_to_interface(&mut self, interface: InterfaceId) -> Option<String> {
        let InterfaceName {
            import_name,
            import_path,
            ..
        } = &self.r#gen.interface_names[&interface];
        if let Identifier::Interface(cur, _) = self.identifier
            && cur == interface
        {
            return None;
        }
        Some(self.deps.import(import_name.clone(), import_path.clone()))
    }

    fn push_str(&mut self, s: &str) {
        self.src.push_str(s);
    }

    fn info(&self, ty: TypeId) -> TypeInfo {
        self.r#gen.types.get(ty)
    }
}

impl<'a> wit_bindgen_core::InterfaceGenerator<'a> for InterfaceGenerator<'a> {
    fn resolve(&self) -> &'a Resolve {
        self.resolve
    }

    fn type_record(
        &mut self,
        id: TypeId,
        _name: &str,
        Record { fields, .. }: &Record,
        docs: &Docs,
    ) {
        let info = self.info(id);
        if let Some(name) = self.name_of(id) {
            self.godoc(docs);
            uwriteln!(self.src, "type {name} struct {{");
            for Field { name, ty, docs, .. } in fields {
                self.godoc(docs);
                self.push_str(&name.to_upper_camel_case());
                self.push_str(" ");
                self.print_opt_ty(ty, true);
                self.push_str("\n");
            }
            self.push_str("}\n");

            let context = self.deps.context();
            let wrpc = self.deps.wrpc();

            // TODO: Print something more useful
            uwriteln!(
                self.src,
                r#"func (v *{name}) String() string {{ return "{name}" }}

func (v *{name}) WriteToIndex(w {wrpc}.ByteWriter) (func({context}.Context, {wrpc}.IndexWriter) error, error) {{
    writes := make(map[uint32]func({context}.Context, {wrpc}.IndexWriter) error, {})"#,
                fields.len(),
            );
            for (i, Field { name, ty, .. }) in fields.iter().enumerate() {
                let fmt = self.deps.fmt();
                let slog = self.deps.slog();
                uwrite!(
                    self.src,
                    r#"{slog}.Debug("writing field", "name", "{name}")
    write{i}, err := "#
                );
                let ident = name.to_upper_camel_case();
                self.print_write_ty(ty, &format!("v.{ident}"), "w");
                uwriteln!(
                    self.src,
                    r#"
    if err != nil {{
        return nil, {fmt}.Errorf("failed to write `{name}` field: %w", err)
    }}
    if write{i} != nil {{
        writes[{i}] = write{i}
    }}"#
                );
            }
            uwriteln!(
                self.src,
                r#"
    return {wrpc}.JoinWrites(writes), nil
}}"#
            );
            if info.error {
                uwriteln!(
                    self.src,
                    r#"func (v *{name}) Error() string {{ return v.String() }}"#
                );
            }
        }
    }

    fn type_resource(&mut self, _id: TypeId, name: &str, docs: &Docs) {
        self.godoc(docs);
        uwriteln!(
            self.src,
            "type {} interface {{}}",
            to_upper_camel_case(name)
        );
    }

    fn type_tuple(&mut self, id: TypeId, _name: &str, tuple: &Tuple, docs: &Docs) {
        if let Some(name) = self.name_of(id) {
            self.godoc(docs);
            uwrite!(self.src, "type {name} = ");
            self.print_tuple(tuple, true);
            self.push_str("\n");
        }
    }

    fn type_flags(&mut self, id: TypeId, _name: &str, ty: &Flags, docs: &Docs) {
        let info = self.info(id);
        if let Some(name) = self.name_of(id) {
            let strings = self.deps.strings();
            let context = self.deps.context();
            let wrpc = self.deps.wrpc();
            let errors = self.deps.errors();
            let io = self.deps.io();

            // Struct
            self.godoc(docs);
            uwriteln!(self.src, "type {name} struct {{");
            for Flag { name, docs, .. } in &ty.flags {
                self.godoc(docs);
                self.push_str(&name.to_upper_camel_case());
                self.push_str(" bool\n");
            }
            self.push_str("}\n");

            // String()
            uwriteln!(self.src, "func (v *{name}) String() string {{");
            uwriteln!(self.src, "flags := make([]string, 0, {})", ty.flags.len());
            for Flag { name, .. } in &ty.flags {
                self.push_str("if v.");
                self.push_str(&name.to_upper_camel_case());
                self.push_str(" {\n");
                uwriteln!(self.src, r#"flags = append(flags, "{name}")"#);
                self.push_str("}\n");
            }
            uwriteln!(self.src, r#"return {strings}.Join(flags, " | ")"#);
            self.push_str("}\n\n");

            // WriteToIndex()
            let mut buf_len = ty.flags.len() / 8;
            if !ty.flags.len().is_multiple_of(8) {
                buf_len += 1;
            }

            uwriteln!(
                self.src,
                r#"func (v *{name}) WriteToIndex(w {wrpc}.ByteWriter) (func({context}.Context, {wrpc}.IndexWriter) error, error) {{
                    var p [{buf_len}]byte
"#
            );

            for (i, Flag { name, .. }) in ty.flags.iter().enumerate() {
                self.push_str("if v.");
                self.push_str(&name.to_upper_camel_case());
                uwriteln!(
                    self.src,
                    r#"{{
        p[{}] |= 1 << {}                
    }}"#,
                    i / 8,
                    i % 8
                );
            }
            uwriteln!(
                self.src,
                r#"
    _, err := w.Write(p[:])
    return nil, err
}}
                "#,
            );

            // ReadFromIndex()
            uwrite!(
                self.src,
                r#"func (v *{name}) ReadFromIndex(r {wrpc}.IndexReader) error {{
        var p [{buf_len}]byte
        if _, err := {io}.ReadFull(r, p[:]); err != nil {{
            return err
        }}
    
    "#
            );

            for (i, Flag { name, .. }) in ty.flags.iter().enumerate() {
                uwriteln!(
                    self.src,
                    "v.{} = p[{}] & (1 << {}) > 0",
                    name.to_upper_camel_case(),
                    i / 8,
                    i % 8
                );
            }

            // Number of meaningful bits in the final byte. When the flag count
            // is an exact multiple of 8 the last byte is fully used, so the
            // shift must be 8 (not `len % 8 == 0`, which would reject every
            // set bit in that byte as "unassociated").
            let last_byte_bits = if ty.flags.is_empty() {
                0
            } else {
                (ty.flags.len() - 1) % 8 + 1
            };
            uwriteln!(
                self.src,
                r#"
    if (p[{}] >> {}) > 0 {{
        return {errors}.New("bit not associated with any flag is set")
    }}"#,
                buf_len - 1,
                last_byte_bits,
            );
            self.push_str("return nil\n}\n");

            // Error()
            if info.error {
                uwriteln!(
                    self.src,
                    r#"func (v *{name}) Error() string {{ return v.String() }}"#
                );
            }
        }
    }

    fn type_variant(&mut self, id: TypeId, _name: &str, variant: &Variant, docs: &Docs) {
        let info = self.info(id);
        if let Some(name) = self.name_of(id) {
            self.godoc(docs);
            uwriteln!(
                self.src,
                r#"type {name} struct {{ payload any; discriminant {name}Discriminant }}"#
            );
            uwriteln!(
                self.src,
                r#"func (v *{name}) Discriminant() {name}Discriminant {{ return v.discriminant }}"#
            );
            uwrite!(self.src, r#"type {name}Discriminant "#);
            self.int_repr(variant.tag());
            self.push_str("\n");
            self.push_str("const (\n");
            for (
                i,
                Case {
                    name: case_name,
                    docs,
                    ..
                },
            ) in variant.cases.iter().enumerate()
            {
                self.godoc(docs);
                self.push_str(&name);
                self.push_str(&case_name.to_upper_camel_case());
                self.push_str(" ");
                self.push_str(&name);
                uwriteln!(self.src, "Discriminant = {i}");
            }
            self.push_str(")\n");

            uwriteln!(
                self.src,
                r#"func (v *{name}) String() string {{ switch v.discriminant {{"#
            );
            for Case {
                name: case_name, ..
            } in &variant.cases
            {
                self.push_str("case ");
                self.push_str(&name);
                self.push_str(&case_name.to_upper_camel_case());
                self.push_str(": return \"");
                self.push_str(case_name);
                self.push_str("\"\n");
            }
            self.push_str("default: panic(\"invalid variant\")\n}\n");
            self.push_str("}\n");

            for Case {
                name: case_name,
                ty,
                docs,
                ..
            } in &variant.cases
            {
                let camel = case_name.to_upper_camel_case();
                self.godoc(docs);
                uwrite!(self.src, r#"func (v *{name}) Get{camel}() ("#);
                if let Some(ty) = ty {
                    self.push_str("payload ");
                    self.print_ty(ty, true);
                    self.push_str(", ");
                }
                self.push_str("ok bool) {\n");
                uwriteln!(
                    self.src,
                    r#"if ok = (v.discriminant == {name}{camel}); !ok {{ return }}"#
                );
                if let Some(ty) = ty {
                    self.push_str("payload, ok = v.payload.(");
                    self.print_ty(ty, true);
                    self.push_str(")\n");
                }
                self.push_str("return\n}\n");

                self.godoc(docs);
                uwrite!(self.src, r#"func (v *{name}) Set{camel}("#);
                if let Some(ty) = ty {
                    self.push_str("payload ");
                    self.print_opt_ty(ty, true);
                }
                uwriteln!(self.src, ") *{name} {{");
                uwriteln!(self.src, "v.discriminant = {name}{camel}");
                if ty.is_some() {
                    self.push_str("v.payload = payload\n");
                } else {
                    self.push_str("v.payload = nil\n");
                }
                self.push_str("return v\n}\n");

                self.godoc(docs);
                uwrite!(self.src, r#"func New{name}{camel}("#);
                if let Some(ty) = ty {
                    self.push_str("payload ");
                    self.print_opt_ty(ty, true);
                }
                uwriteln!(self.src, ") *{name} {{");
                uwriteln!(self.src, "return (&{name}{{}}).Set{camel}(");
                if ty.is_some() {
                    self.push_str("payload");
                }
                self.push_str(")\n}\n");
            }

            if info.error {
                uwriteln!(
                    self.src,
                    r#"func (v *{name}) Error() string {{ return v.String() }}"#
                );
            }

            let context = self.deps.context();
            let fmt = self.deps.fmt();
            let wrpc = self.deps.wrpc();
            uwriteln!(
                self.src,
                "func (v *{name}) WriteToIndex(w {wrpc}.ByteWriter) (func({context}.Context, {wrpc}.IndexWriter) error, error) {{",
            );
            self.push_str("if err := ");
            self.print_write_discriminant(variant.tag(), "v.discriminant", "w");
            self.push_str("; err != nil { return nil, ");
            self.push_str(fmt);
            self.push_str(".Errorf(\"failed to write discriminant: %w\", err)\n}\n");
            let errors = self.deps.errors();
            self.push_str("switch v.discriminant {\n");
            for Case {
                name: case_name,
                ty,
                ..
            } in &variant.cases
            {
                self.push_str("case ");
                self.push_str(&name);
                self.push_str(&case_name.to_upper_camel_case());
                self.push_str(":\n");
                if let Some(ty) = ty {
                    self.push_str("payload, ok := v.payload.(");
                    self.print_opt_ty(ty, true);
                    self.push_str(")\n");
                    self.push_str("if !ok { return nil, ");
                    self.push_str(errors);
                    self.push_str(".New(\"invalid payload\") }\n");
                    self.push_str("write, err := ");
                    self.print_write_ty(ty, "payload", "w");
                    self.push_str("\n");
                    self.push_str("if err != nil { return nil, ");
                    self.push_str(fmt);
                    self.push_str(".Errorf(\"failed to write payload: %w\", err)\n}\n");
                    self.push_str("return write, nil\n");
                }
            }
            self.push_str("default: return nil, ");
            self.push_str(errors);
            self.push_str(".New(\"invalid variant\")\n}\n");
            self.push_str("return nil, nil\n");
            self.push_str("}\n");
        }
    }

    fn type_option(&mut self, id: TypeId, _name: &str, payload: &Type, docs: &Docs) {
        if let Some(name) = self.name_of(id) {
            self.godoc(docs);
            uwrite!(self.src, "type {name} = ");
            self.print_option(payload, true);
            self.push_str("\n");
        }
    }

    fn type_result(&mut self, id: TypeId, _name: &str, result: &Result_, docs: &Docs) {
        if let Some(name) = self.name_of(id) {
            self.godoc(docs);
            uwrite!(self.src, "type {name} = ");
            self.print_result(result);
            self.push_str("\n");
        }
    }

    fn type_enum(&mut self, id: TypeId, _name: &str, enum_: &Enum, docs: &Docs) {
        let info = self.info(id);
        if let Some(name) = self.name_of(id) {
            self.godoc(docs);
            uwrite!(self.src, r#"type {name} "#);
            self.int_repr(enum_.tag());
            self.push_str("\n");
            self.push_str("const (\n");
            for (
                i,
                EnumCase {
                    name: case_name,
                    docs,
                    ..
                },
            ) in enum_.cases.iter().enumerate()
            {
                self.godoc(docs);
                self.push_str(&name);
                self.push_str("_");
                self.push_str(&case_name.to_upper_camel_case());
                self.push_str(" ");
                self.push_str(&name);
                uwriteln!(self.src, " = {i}");
            }
            self.push_str(")\n");

            uwriteln!(
                self.src,
                r#"func (v {name}) String() string {{ switch v {{"#
            );
            for EnumCase {
                name: case_name, ..
            } in &enum_.cases
            {
                self.push_str("case ");
                self.push_str(&name);
                self.push_str("_");
                self.push_str(&case_name.to_upper_camel_case());
                self.push_str(": return \"");
                self.push_str(case_name);
                self.push_str("\"\n");
            }
            self.push_str("default: panic(\"invalid enum\")\n}\n");
            self.push_str("}\n");

            if info.error {
                uwriteln!(
                    self.src,
                    r#"func (v {name}) Error() string {{ return v.String() }}"#
                );
            }

            let context = self.deps.context();
            let fmt = self.deps.fmt();
            let wrpc = self.deps.wrpc();
            uwriteln!(
                self.src,
                "func (v {name}) WriteToIndex(w {wrpc}.ByteWriter) (func({context}.Context, {wrpc}.IndexWriter) error, error) {{",
            );
            self.push_str("if err := ");
            self.print_write_discriminant(enum_.tag(), "v", "w");
            self.push_str("; err != nil { return nil, ");
            self.push_str(fmt);
            self.push_str(".Errorf(\"failed to write discriminant: %w\", err)\n}\n");
            self.push_str("return nil, nil\n");
            self.push_str("}\n");
        }
    }

    fn type_alias(&mut self, id: TypeId, _name: &str, ty: &Type, docs: &Docs) {
        if let Some(name) = self.name_of(id) {
            self.godoc(docs);
            uwrite!(self.src, "type {name} = ");
            self.print_ty(ty, true);
            self.push_str("\n");
        }
    }

    fn type_list(&mut self, id: TypeId, _name: &str, ty: &Type, docs: &Docs) {
        if let Some(name) = self.name_of(id) {
            self.godoc(docs);
            uwrite!(self.src, "type {name} = ");
            self.print_list(ty);
            self.push_str("\n");
        }
    }

    fn type_fixed_length_list(
        &mut self,
        _id: TypeId,
        _name: &str,
        _ty: &Type,
        _size: u32,
        _docs: &Docs,
    ) {
        panic!("unsupported type: fixed length list")
    }

    fn type_map(&mut self, _id: TypeId, _name: &str, _key: &Type, _value: &Type, _docs: &Docs) {
        panic!("unsupported type: map")
    }

    fn type_builtin(&mut self, _id: TypeId, name: &str, ty: &Type, docs: &Docs) {
        self.godoc(docs);
        uwrite!(self.src, "type {} = ", name.to_upper_camel_case());
        self.print_ty(ty, true);
        self.src.push_str("\n");
    }

    fn type_future(&mut self, id: TypeId, _name: &str, ty: &Option<Type>, docs: &Docs) {
        if let Some(name) = self.name_of(id) {
            self.godoc(docs);
            uwrite!(self.src, "type {name} = ");
            self.print_future(ty);
            self.push_str("\n");
        }
    }

    fn type_stream(&mut self, id: TypeId, _name: &str, ty: &Option<Type>, docs: &Docs) {
        if let Some(name) = self.name_of(id) {
            self.godoc(docs);
            uwrite!(self.src, "type {name} = ");
            self.print_stream(ty);
            self.push_str("\n");
        }
    }
}
