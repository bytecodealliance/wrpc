use crate::{
    to_go_ident, to_package_ident, to_upper_camel_case, Deps, FnSig, GoWrpc, Identifier,
    InterfaceName,
};
use heck::ToUpperCamelCase;
use std::collections::{BTreeMap, VecDeque};
use std::fmt::Write as _;
use std::mem;
use wit_bindgen_core::{
    dealias, uwrite, uwriteln,
    wit_parser::{
        Case, Docs, Enum, EnumCase, Field, Flag, Flags, Function, FunctionKind, Handle, Int,
        InterfaceId, Record, Resolve, Result_, Stream, Tuple, Type, TypeDefKind, TypeId, TypeOwner,
        Variant, World, WorldKey,
    },
    Source, TypeInfo,
};

pub struct InterfaceGenerator<'a> {
    pub src: Source,
    pub(super) identifier: Identifier<'a>,
    pub in_import: bool,
    pub(super) gen: &'a mut GoWrpc,
    pub resolve: &'a Resolve,
    pub deps: Deps,
}

impl InterfaceGenerator<'_> {
    fn print_read_ty(&mut self, ty: &Type, reader: &str, path: &str) {
        // NOTE: u{16,32,64} decoding adapted from
        // https://cs.opensource.google/go/go/+/refs/tags/go1.22.2:src/encoding/binary/varint.go;l=128-153
        // NOTE: s{16,32,64} decoding adapted from
        // https://github.com/go-delve/delve/blob/26799555e5518e8a9fe2d68e02379257ebda4dd2/pkg/dwarf/leb128/decode.go#L51-L81
        match ty {
            Type::Id(t) => self.print_read_tyid(*t, reader, path),
            Type::Bool => uwrite!(
                self.src,
                r#"func(r {wrpc}.ByteReader) (bool, error) {{
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
                fmt = self.deps.fmt(),
                slog = self.deps.slog(),
                wrpc = self.deps.wrpc(),
            ),
            Type::U8 => uwrite!(
                self.src,
                r#"func(r {wrpc}.ByteReader) (uint8, error) {{
    {slog}.Debug("reading u8 byte")
    v, err := r.ReadByte()
    if err != nil {{
        return 0, {fmt}.Errorf("failed to read u8 byte: %w", err)
    }}
    return v, nil
}}({reader})"#,
                fmt = self.deps.fmt(),
                slog = self.deps.slog(),
                wrpc = self.deps.wrpc(),
            ),
            Type::U16 => uwrite!(
                self.src,
                r#"func(r {wrpc}.ByteReader) (uint16, error) {{
	var x uint16
	var s uint
	for i := 0; i < 3; i++ {{
        {slog}.Debug("reading u16 byte", "i", i)
		b, err := r.ReadByte()
		if err != nil {{
			if i > 0 && err == {io}.EOF {{
				err = {io}.ErrUnexpectedEOF
			}}
			return x, {fmt}.Errorf("failed to read u16 byte: %w", err)
		}}
		if b < 0x80 {{
			if i == 2 && b > 1 {{
				return x, {errors}.New("varint overflows a 16-bit integer")
			}}
			return x | uint16(b)<<s, nil
		}}
		x |= uint16(b&0x7f) << s
		s += 7
	}}
	return x, {errors}.New("varint overflows a 16-bit integer")
}}({reader})"#,
                errors = self.deps.errors(),
                fmt = self.deps.fmt(),
                io = self.deps.io(),
                slog = self.deps.slog(),
                wrpc = self.deps.wrpc(),
            ),
            Type::U32 => uwrite!(
                self.src,
                r#"func(r {wrpc}.ByteReader) (uint32, error) {{
	var x uint32
	var s uint
	for i := 0; i < 5; i++ {{
        {slog}.Debug("reading u32 byte", "i", i)
		b, err := r.ReadByte()
		if err != nil {{
			if i > 0 && err == {io}.EOF {{
				err = {io}.ErrUnexpectedEOF
			}}
			return x, {fmt}.Errorf("failed to read u32 byte: %w", err)
		}}
		if b < 0x80 {{
			if i == 4 && b > 1 {{
				return x, {errors}.New("varint overflows a 32-bit integer")
			}}
			return x | uint32(b)<<s, nil
		}}
		x |= uint32(b&0x7f) << s
		s += 7
	}}
	return x, {errors}.New("varint overflows a 32-bit integer")
}}({reader})"#,
                errors = self.deps.errors(),
                fmt = self.deps.fmt(),
                io = self.deps.io(),
                slog = self.deps.slog(),
                wrpc = self.deps.wrpc(),
            ),
            Type::U64 => uwrite!(
                self.src,
                r#"func(r {wrpc}.ByteReader) (uint64, error) {{
	var x uint64
	var s uint
	for i := 0; i < 10; i++ {{
        {slog}.Debug("reading u64 byte", "i", i)
		b, err := r.ReadByte()
		if err != nil {{
			if i > 0 && err == {io}.EOF {{
				err = {io}.ErrUnexpectedEOF
			}}
			return x, {fmt}.Errorf("failed to read u64 byte: %w", err)
		}}
		if b < 0x80 {{
			if i == 9 && b > 1 {{
				return x, {errors}.New("varint overflows a 64-bit integer")
			}}
			return x | uint64(b)<<s, nil
		}}
		x |= uint64(b&0x7f) << s
		s += 7
	}}
	return x, {errors}.New("varint overflows a 64-bit integer")
}}({reader})"#,
                errors = self.deps.errors(),
                fmt = self.deps.fmt(),
                io = self.deps.io(),
                slog = self.deps.slog(),
                wrpc = self.deps.wrpc(),
            ),
            Type::S8 => uwrite!(
                self.src,
                r#"func(r {wrpc}.ByteReader) (int8, error) {{
    {slog}.Debug("reading s8 byte")
    v, err := r.ReadByte()
    if err != nil {{
        return 0, {fmt}.Errorf("failed to read s8 byte: %w", err)
    }}
    return int8(v), nil
}}({reader})"#,
                fmt = self.deps.fmt(),
                slog = self.deps.slog(),
                wrpc = self.deps.wrpc(),
            ),
            Type::S16 => uwrite!(
                self.src,
                r#"func(r {wrpc}.ByteReader) (int16, error) {{
    var (
		b      byte
		err    error
		result int16
		shift  uint16
		length uint32
	)
	for {{
        {slog}.Debug("reading s16 byte")
		b, err = r.ReadByte()
        if err != nil {{
            return 0, {fmt}.Errorf("failed to read s16 byte: %w", err)
        }}
		length++

		result |= (int16(b) & 0x7f) << shift
		shift += 7
		if b&0x80 == 0 {{
			break
		}}
	}}
	if (shift < 8*uint16(length)) && (b&0x40 > 0) {{
		result |= -(1 << shift)
	}}
	return result, nil
}}({reader})"#,
                fmt = self.deps.fmt(),
                slog = self.deps.slog(),
                wrpc = self.deps.wrpc(),
            ),
            Type::S32 => uwrite!(
                self.src,
                r#"func(r {wrpc}.ByteReader) (int32, error) {{
    var (
		b      byte
		err    error
		result int32
		shift  uint32
		length uint32
	)
	for {{
        {slog}.Debug("reading s32 byte")
		b, err = r.ReadByte()
        if err != nil {{
            return 0, {fmt}.Errorf("failed to read s32 byte: %w", err)
        }}
		length++

		result |= (int32(b) & 0x7f) << shift
		shift += 7
		if b&0x80 == 0 {{
			break
		}}
	}}
	if (shift < 8*uint32(length)) && (b&0x40 > 0) {{
		result |= -(1 << shift)
	}}
	return result, nil
}}({reader})"#,
                fmt = self.deps.fmt(),
                slog = self.deps.slog(),
                wrpc = self.deps.wrpc(),
            ),
            Type::S64 => uwrite!(
                self.src,
                r#"func(r {wrpc}.ByteReader) (int64, error) {{
    var (
		b      byte
		err    error
		result int64
		shift  uint64
		length uint32
	)
	for {{
        {slog}.Debug("reading s64 byte")
		b, err = r.ReadByte()
        if err != nil {{
            return 0, {fmt}.Errorf("failed to read s64 byte: %w", err)
        }}
		length++

		result |= (int64(b) & 0x7f) << shift
		shift += 7
		if b&0x80 == 0 {{
			break
		}}
	}}
	if (shift < 8*uint64(length)) && (b&0x40 > 0) {{
		result |= -(1 << shift)
	}}
	return result, nil
}}({reader})"#,
                fmt = self.deps.fmt(),
                slog = self.deps.slog(),
                wrpc = self.deps.wrpc(),
            ),
            Type::F32 => uwrite!(
                self.src,
                r#"func(r {wrpc}.ByteReader) (float32, error) {{
    var b [4]byte
    {slog}.Debug("reading f32 bytes")
    _, err := r.Read(b[:])
    if err != nil {{
        return 0, {fmt}.Errorf("failed to read f32: %w", err)
    }}
    return {math}.Float32frombits({binary}.LittleEndian.Uint32(b[:])), nil
}}({reader})"#,
                binary = self.deps.binary(),
                fmt = self.deps.fmt(),
                math = self.deps.math(),
                slog = self.deps.slog(),
                wrpc = self.deps.wrpc(),
            ),
            Type::F64 => uwrite!(
                self.src,
                r#"func(r {wrpc}.ByteReader) (float64, error) {{
    var b [8]byte
    {slog}.Debug("reading f64 bytes")
    _, err := r.Read(b[:])
    if err != nil {{
        return 0, {fmt}.Errorf("failed to read f64: %w", err)
    }}
    return {math}.Float64frombits({binary}.LittleEndian.Uint64(b[:])), nil
}}({reader})"#,
                binary = self.deps.binary(),
                fmt = self.deps.fmt(),
                math = self.deps.math(),
                slog = self.deps.slog(),
                wrpc = self.deps.wrpc(),
            ),
            Type::Char => uwrite!(
                self.src,
                r#"func(r {wrpc}.ByteReader) (rune, error) {{
	var x uint32
	var s uint
	for i := 0; i < 5; i++ {{
        {slog}.Debug("reading char byte", "i", i)
		b, err := r.ReadByte()
		if err != nil {{
			if i > 0 && err == {io}.EOF {{
				err = {io}.ErrUnexpectedEOF
			}}
			return {utf8}.RuneError, {fmt}.Errorf("failed to read char byte: %w", err)
		}}
		if b < 0x80 {{
			if i == 4 && b > 1 {{
				return {utf8}.RuneError, {errors}.New("char overflows a 32-bit integer")
			}}
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
                errors = self.deps.errors(),
                fmt = self.deps.fmt(),
                io = self.deps.io(),
                slog = self.deps.slog(),
                utf8 = self.deps.utf8(),
                wrpc = self.deps.wrpc(),
            ),
            Type::String => uwrite!(
                self.src,
                r#"func(r {wrpc}.ByteReader) (string, error) {{
	var x uint32
	var s uint
	for i := 0; i < 5; i++ {{
        {slog}.Debug("reading string length byte", "i", i)
		b, err := r.ReadByte()
		if err != nil {{
			if i > 0 && err == {io}.EOF {{
				err = {io}.ErrUnexpectedEOF
			}}
			return "", {fmt}.Errorf("failed to read string length byte: %w", err)
		}}
		if b < 0x80 {{
			if i == 4 && b > 1 {{
				return "", {errors}.New("string length overflows a 32-bit integer")
			}}
            x = x | uint32(b)<<s
            buf := make([]byte, x)
            {slog}.Debug("reading string bytes", "len", x)
	        _, err = r.Read(buf)
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
                errors = self.deps.errors(),
                fmt = self.deps.fmt(),
                io = self.deps.io(),
                slog = self.deps.slog(),
                utf8 = self.deps.utf8(),
                wrpc = self.deps.wrpc(),
            ),
        }
    }

    fn print_read_tyid(&mut self, id: TypeId, reader: &str, path: &str) {
        let ty = &self.resolve.types[id];
        if let Some(ref name) = ty.name {
            let read = self.type_path_with_name(id, format!("Read{}", to_upper_camel_case(name)));
            uwrite!(self.src, "{read}({reader})");
            return;
        }

        match &ty.kind {
            TypeDefKind::List(ty) => {
                let fmt = self.deps.fmt();
                let io = self.deps.io();
                let errors = self.deps.errors();
                let slog = self.deps.slog();
                let wrpc = self.deps.wrpc();
                uwrite!(self.src, "func(r {wrpc}.ByteReader) (");
                self.print_list(ty);
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
			return nil, {fmt}.Errorf("failed to read list length byte: %w", err)
		}}
		if b < 0x80 {{
			if i == 4 && b > 1 {{
				return nil, {errors}.New("list length overflows a 32-bit integer")
			}}
            x = x | uint32(b)<<s
            vs := make("#,
                );
                self.print_list(ty);
                uwrite!(
                    self.src,
                    r#", x)
            for i := range vs {{
                {slog}.Debug("reading list element", "i", i)
                vs[i], err = "#,
                );
                self.print_read_ty(ty, "r", &format!("{path}, i"));
                self.push_str("\n");
                uwrite!(
                    self.src,
                    r#"if err != nil {{
			        return nil, {fmt}.Errorf("failed to read list element %d: %w", i, err)
                }}
            }}
            return vs, nil
		}}
		x |= uint32(b&0x7f) << s
		s += 7
	}}
	return nil, {errors}.New("list length overflows a 32-bit integer")
}}({reader})"#,
                );
            }

            TypeDefKind::Option(ty) => {
                let fmt = self.deps.fmt();
                let slog = self.deps.slog();
                let wrpc = self.deps.wrpc();
                uwrite!(self.src, "func(r {wrpc}.ByteReader) (");
                self.print_option(ty, true);
                uwrite!(
                    self.src,
                    r#", error) {{
    {slog}.Debug("reading option status byte")
	status, err := r.ReadByte()
	if err != nil {{
		return nil, {fmt}.Errorf("failed to read option status byte: %w", err)
	}}
	switch status {{
	case 0:
		return nil, nil
	case 1:
        {slog}.Debug("reading `option::some` payload")
	    v, err := "#,
                );
                self.print_read_ty(ty, "r", &format!("{path}, 1"));
                self.push_str("\n");
                uwrite!(
                    self.src,
                    r#"if err != nil {{
	    	return nil, {fmt}.Errorf("failed to read `option::some` value: %w", err)
	    }}
	    return "#,
                );
                self.result_element_ptr(ty, false);
                uwrite!(
                    self.src,
                    r#"v, nil
	default:
		return nil, {fmt}.Errorf("invalid option status byte %d", status)
	}}
}}({reader})"#,
                );
            }

            TypeDefKind::Result(ty) => {
                let fmt = self.deps.fmt();
                let slog = self.deps.slog();
                let wrpc = self.deps.wrpc();
                uwrite!(self.src, "func(r {wrpc}.ByteReader) (*");
                self.print_result(ty);
                uwriteln!(
                    self.src,
                    r#", error) {{
    {slog}.Debug("reading result status byte")
	status, err := r.ReadByte()
	if err != nil {{
		return nil, {fmt}.Errorf("failed to read result status byte: %w", err)
	}}"#,
                );
                self.push_str("switch status {\n");
                self.push_str("case 0:\n");
                if let Some(ref ty) = ty.ok {
                    uwriteln!(self.src, r#"{slog}.Debug("reading `result::ok` payload")"#);
                    self.push_str("v, err := ");
                    self.print_read_ty(ty, "r", &format!("{path}, 0"));
                    self.push_str("\n");
                    uwriteln!(
                        self.src,
                        r#"if err != nil {{
	    	return nil, fmt.Errorf("failed to read `result::ok` value: %w", err)
	    }}"#,
                    );
                } else {
                    self.push_str("var v struct{}\n");
                }
                self.push_str("return &");
                self.print_result(ty);
                self.push_str("{ Ok: &v }, nil\n");
                self.push_str("case 1:\n");
                if let Some(ref err) = ty.err {
                    uwriteln!(self.src, r#"{slog}.Debug("reading `result::err` payload")"#);
                    self.push_str("v, err := ");
                    self.print_read_ty(err, "r", &format!("{path}, 1"));
                    self.push_str("\n");
                    uwriteln!(
                        self.src,
                        r#"if err != nil {{
	    	return nil, {fmt}.Errorf("failed to read `result::err` value: %w", err)
	    }}"#,
                    );
                    self.push_str("return &");
                    self.print_result(ty);
                    self.push_str("{ Err: ");
                    self.result_element_ptr(err, false);
                } else {
                    self.push_str("var v struct{}\n");
                    self.push_str("return &");
                    self.print_result(ty);
                    self.push_str("{ Err: &");
                }
                uwrite!(
                    self.src,
                    r#"v }}, nil
	default:
		return nil, {fmt}.Errorf("invalid result status byte %d", status)
	}}
}}({reader})"#,
                );
            }

            TypeDefKind::Variant(_) => panic!("unsupported anonymous variant"),

            TypeDefKind::Tuple(ty) => match ty.types.as_slice() {
                [] => self.push_str("struct{}{}, nil"),
                [ty] => self.print_read_ty(ty, reader, &format!("{path}, 0")),
                _ => {
                    let wrpc = self.deps.wrpc();

                    uwrite!(self.src, "func(r {wrpc}.ByteReader) (");
                    self.print_tuple(ty, true);
                    self.push_str(", error) {\n");
                    self.push_str("v := ");
                    self.print_tuple(ty, false);
                    self.push_str("{}\n");
                    self.push_str("var err error\n");
                    for (i, ty) in ty.types.iter().enumerate() {
                        let fmt = self.deps.fmt();
                        let slog = self.deps.slog();
                        uwrite!(
                            self.src,
                            r#"{slog}.Debug("reading tuple element {i}")
        v.V{i}, err = "#
                        );
                        self.print_read_ty(ty, "r", &format!("{path}, {i}"));
                        self.push_str("\n");
                        uwriteln!(
                            self.src,
                            r#"if err != nil {{
		    return nil, {fmt}.Errorf("failed to read tuple element {i}: %w", err)
	    }}"#
                        );
                    }
                    self.push_str("return v, nil\n");
                    uwrite!(self.src, "}}({reader})");
                }
            },
            TypeDefKind::Resource => {
                panic!("unsupported anonymous type reference: resource")
            }
            TypeDefKind::Record(_) => {
                panic!("unsupported anonymous type reference: record")
            }
            TypeDefKind::Flags(_) => {
                panic!("unsupported anonymous type reference: flags")
            }
            TypeDefKind::Enum(_) => {
                panic!("unsupported anonymous type reference: enum")
            }
            TypeDefKind::Future(_ty) => uwrite!(
                self.src,
                r#"nil, {errors}.New("reading futures not supported yet")"#,
                errors = self.deps.errors(),
            ),
            TypeDefKind::Stream(ty) => match ty.element {
                Some(Type::U8) => uwrite!(
                    self.src,
                    "{wrpc}.ReadByteStream({reader}, {path})",
                    wrpc = self.deps.wrpc(),
                ),
                _ => uwrite!(
                    self.src,
                    r#"nil, {errors}.New("reading non-byte streams not supported yet")"#,
                    errors = self.deps.errors(),
                ),
            },

            TypeDefKind::Handle(Handle::Own(_ty)) => uwrite!(
                self.src,
                r#"0, {errors}.New("reading owned handles not supported yet")"#,
                errors = self.deps.errors(),
            ),

            TypeDefKind::Handle(Handle::Borrow(_ty)) => uwrite!(
                self.src,
                r#"0, {errors}.New("reading borrowed handles not supported yet")"#,
                errors = self.deps.errors(),
            ),

            TypeDefKind::Type(t) => self.print_read_ty(t, reader, path),

            TypeDefKind::Unknown => unreachable!(),
        }
    }

    fn print_write_ty(&mut self, ty: &Type, name: &str, writer: &str) {
        // NOTE: s{16,32,64} encoding adapted from
        // https://github.com/go-delve/delve/blob/26799555e5518e8a9fe2d68e02379257ebda4dd2/pkg/dwarf/leb128/encode.go#L23-L42
        match ty {
            Type::Id(t) => self.print_write_tyid(*t, name, writer),
            Type::Bool => uwrite!(
                self.src,
                r#"func(v bool, w {wrpc}.ByteWriter) (func({wrpc}.IndexWriter) error, error) {{
                if !v {{
                    {slog}.Debug("writing `false` byte")
                    return nil, w.WriteByte(0)
                }}
                {slog}.Debug("writing `true` byte")
                return nil, w.WriteByte(1)
            }}({name}, {writer})"#,
                slog = self.deps.slog(),
                wrpc = self.deps.wrpc(),
            ),
            Type::U8 => uwrite!(
                self.src,
                r#"func(v uint8, w {wrpc}.ByteWriter) (func({wrpc}.IndexWriter) error, error) {{
                {slog}.Debug("writing u8 byte")
                return nil, w.WriteByte(v)
            }}({name}, {writer})"#,
                slog = self.deps.slog(),
                wrpc = self.deps.wrpc(),
            ),
            Type::U16 => uwrite!(
                self.src,
                r#"func(v uint16, w {wrpc}.ByteWriter) (func({wrpc}.IndexWriter) error, error) {{
	            b := make([]byte, {binary}.MaxVarintLen16)
	            i := {binary}.PutUvarint(b, uint64(v))
                {slog}.Debug("writing u16")
	            _, err := w.Write(b[:i])
	            return nil, err
            }}({name}, {writer})"#,
                binary = self.deps.binary(),
                slog = self.deps.slog(),
                wrpc = self.deps.wrpc(),
            ),
            Type::U32 => uwrite!(
                self.src,
                r#"func(v uint32, w {wrpc}.ByteWriter) (func({wrpc}.IndexWriter) error, error) {{
	            b := make([]byte, {binary}.MaxVarintLen32)
	            i := {binary}.PutUvarint(b, uint64(v))
                {slog}.Debug("writing u32")
	            _, err := w.Write(b[:i])
	            return nil, err
            }}({name}, {writer})"#,
                binary = self.deps.binary(),
                slog = self.deps.slog(),
                wrpc = self.deps.wrpc(),
            ),
            Type::U64 => uwrite!(
                self.src,
                r#"func(v uint64, w {wrpc}.ByteWriter) (func({wrpc}.IndexWriter) error, error) {{
	            b := make([]byte, {binary}.MaxVarintLen64)
	            i := {binary}.PutUvarint(b, uint64(v))
                {slog}.Debug("writing u64")
	            _, err := w.Write(b[:i])
	            return nil, err
            }}({name}, {writer})"#,
                binary = self.deps.binary(),
                slog = self.deps.slog(),
                wrpc = self.deps.wrpc(),
            ),
            Type::S8 => uwrite!(
                self.src,
                r#"func(v int8, w {wrpc}.ByteWriter) (func({wrpc}.IndexWriter) error, error) {{
                {slog}.Debug("writing s8 byte")
                return nil, w.WriteByte(byte(v))
            }}({name}, {writer})"#,
                slog = self.deps.slog(),
                wrpc = self.deps.wrpc(),
            ),
            Type::S16 => uwrite!(
                self.src,
                r#"func(v int16, w {wrpc}.ByteWriter) (func({wrpc}.IndexWriter) error, error) {{
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
                    if err := w.WriteByte(b); err != nil {{
	    		        return nil, {fmt}.Errorf("failed to write `s16` byte: %w", err)
                    }}
	            	if last {{
	            		return nil, nil
	            	}}
	            }}
            }}({name}, {writer})"#,
                fmt = self.deps.fmt(),
                slog = self.deps.slog(),
                wrpc = self.deps.wrpc(),
            ),
            Type::S32 => uwrite!(
                self.src,
                r#"func(v int32, w {wrpc}.ByteWriter) (func({wrpc}.IndexWriter) error, error) {{
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
                    if err := w.WriteByte(b); err != nil {{
	    		        return nil, {fmt}.Errorf("failed to write `s32` byte: %w", err)
                    }}
	            	if last {{
	            		return nil, nil
	            	}}
	            }}
            }}({name}, {writer})"#,
                fmt = self.deps.fmt(),
                slog = self.deps.slog(),
                wrpc = self.deps.wrpc(),
            ),
            Type::S64 => uwrite!(
                self.src,
                r#"func(v int64, w {wrpc}.ByteWriter) (func({wrpc}.IndexWriter) error, error) {{
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
                    if err := w.WriteByte(b); err != nil {{
	    		        return nil, {fmt}.Errorf("failed to write `s64` byte: %w", err)
                    }}
	            	if last {{
	            		return nil, nil
	            	}}
	            }}
            }}({name}, {writer})"#,
                fmt = self.deps.fmt(),
                slog = self.deps.slog(),
                wrpc = self.deps.wrpc(),
            ),
            Type::F32 => uwrite!(
                self.src,
                r#"func(v float32, w {wrpc}.ByteWriter) (func({wrpc}.IndexWriter) error, error) {{
                b := make([]byte, 4)
                {binary}.LittleEndian.PutUint32(b, {math}.Float32bits(v))
                {slog}.Debug("writing f32")
	            _, err := w.Write(b)
	            return nil, err
            }}({name}, {writer})"#,
                binary = self.deps.binary(),
                math = self.deps.math(),
                slog = self.deps.slog(),
                wrpc = self.deps.wrpc(),
            ),
            Type::F64 => uwrite!(
                self.src,
                r#"func(v float64, w {wrpc}.ByteWriter) (func({wrpc}.IndexWriter) error, error) {{
                b := make([]byte, 8)
                {binary}.LittleEndian.PutUint64(b, {math}.Float64bits(v))
                {slog}.Debug("writing f64")
	            _, err := w.Write(b)
	            return nil, err
            }}({name}, {writer})"#,
                binary = self.deps.binary(),
                math = self.deps.math(),
                slog = self.deps.slog(),
                wrpc = self.deps.wrpc(),
            ),
            Type::Char => uwrite!(
                self.src,
                r#"func(v rune, w {wrpc}.ByteWriter) (func({wrpc}.IndexWriter) error, error) {{
	            b := make([]byte, {binary}.MaxVarintLen32)
	            i := {binary}.PutUvarint(b, uint64(v))
                {slog}.Debug("writing char")
	            _, err := w.Write(b[:i])
	            return nil, err
            }}({name}, {writer})"#,
                binary = self.deps.binary(),
                slog = self.deps.slog(),
                wrpc = self.deps.wrpc(),
            ),
            Type::String => {
                let binary = self.deps.binary();
                let fmt = self.deps.fmt();
                let math = self.deps.math();
                let slog = self.deps.slog();
                let wrpc = self.deps.wrpc();

                uwrite!(
                    self.src,
                    r#"func(v string, w {wrpc}.ByteWriter) (func({wrpc}.IndexWriter) error, error) {{
	            n := len(v)
	            if n > {math}.MaxUint32 {{
	            	return nil, {fmt}.Errorf("string byte length of %d overflows a 32-bit integer", n)
	            }}
	            if err := func(v int, w {wrpc}.ByteWriter) error {{
	                b := make([]byte, {binary}.MaxVarintLen32)
	                i := {binary}.PutUvarint(b, uint64(v))
	                {slog}.Debug("writing string byte length", "len", n)
	                _, err := w.Write(b[:i])
	                return err
                }}(n, w); err != nil {{
                	return nil, {fmt}.Errorf("failed to write string length of %d: %w", n, err)
                }}
                {slog}.Debug("writing string bytes")
                _, err := w.Write([]byte(v))
                if err != nil {{
                	return nil, {fmt}.Errorf("failed to write string bytes: %w", err)
                }}
                return nil, nil
                }}({name}, {writer})"#
                );
            }
        }
    }

    fn print_read_discriminant(&mut self, repr: Int, reader: &str) {
        match repr {
            Int::U8 => {
                uwrite!(
                    self.src,
                    r#"func(r {wrpc}.ByteReader) (uint8, error) {{
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
		if b < 0x80 {{
			if i == 2 && b > 1 {{
				return x, {errors}.New("discriminant overflows a 8-bit integer")
			}}
			return x | uint8(b)<<s, nil
		}}
		x |= uint8(b&0x7f) << s
		s += 7
	}}
	return x, {errors}.New("discriminant overflows a 8-bit integer")
}}({reader})"#,
                    errors = self.deps.errors(),
                    fmt = self.deps.fmt(),
                    io = self.deps.io(),
                    slog = self.deps.slog(),
                    wrpc = self.deps.wrpc(),
                );
            }
            Int::U16 => {
                self.print_read_ty(&Type::U16, reader, "");
            }
            Int::U32 => {
                self.print_read_ty(&Type::U32, reader, "");
            }
            Int::U64 => {
                self.print_read_ty(&Type::U64, reader, "");
            }
        }
    }

    fn print_write_discriminant(&mut self, repr: Int, name: &str, writer: &str) {
        match repr {
            Int::U8 => uwrite!(
                self.src,
                r#"func(v uint8, w {wrpc}.ByteWriter) error {{
	            b := make([]byte, 2)
	            i := {binary}.PutUvarint(b, uint64(v))
                {slog}.Debug("writing u8 discriminant")
	            _, err := w.Write(b[:i])
	            return err
            }}(uint8({name}), {writer})"#,
                binary = self.deps.binary(),
                slog = self.deps.slog(),
                wrpc = self.deps.wrpc(),
            ),
            Int::U16 => uwrite!(
                self.src,
                r#"func(v uint16, w {wrpc}.ByteWriter) error {{
	            b := make([]byte, {binary}.MaxVarintLen16)
	            i := {binary}.PutUvarint(b, uint64(v))
                {slog}.Debug("writing u16 discriminant")
	            _, err := w.Write(b[:i])
	            return err
            }}(uint16({name}), {writer})"#,
                binary = self.deps.binary(),
                slog = self.deps.slog(),
                wrpc = self.deps.wrpc(),
            ),
            Int::U32 => uwrite!(
                self.src,
                r#"func(v uint32, w {wrpc}.ByteWriter) (any, error) {{
	            b := make([]byte, {binary}.MaxVarintLen32)
	            i := {binary}.PutUvarint(b, uint64(v))
                {slog}.Debug("writing u32 discriminant")
	            _, err := w.Write(b[:i])
	            return err
            }}(uint32({name}), {writer})"#,
                binary = self.deps.binary(),
                slog = self.deps.slog(),
                wrpc = self.deps.wrpc(),
            ),
            Int::U64 => uwrite!(
                self.src,
                r#"func(v uint64, w {wrpc}.ByteWriter) (any, error) {{
	            b := make([]byte, {binary}.MaxVarintLen64)
	            i := {binary}.PutUvarint(b, uint64(v))
                {slog}.Debug("writing u64 discriminant")
	            _, err := w.Write(b[:i])
	            return err
            }}(uint64({name}), {writer})"#,
                binary = self.deps.binary(),
                slog = self.deps.slog(),
                wrpc = self.deps.wrpc(),
            ),
        }
    }

    fn print_write_tyid(&mut self, id: TypeId, name: &str, writer: &str) {
        let ty = &self.resolve.types[id];
        if ty.name.is_some() {
            // TODO: Support async
            uwrite!(self.src, "({name}).WriteToIndex({writer})");
            return;
        }

        match &ty.kind {
            TypeDefKind::List(ty) => {
                let binary = self.deps.binary();
                let errgroup = self.deps.errgroup();
                let fmt = self.deps.fmt();
                let math = self.deps.math();
                let slog = self.deps.slog();
                let wrpc = self.deps.wrpc();

                self.push_str("func(v ");
                self.print_list(ty);
                uwrite!(
                    self.src,
                    r#", w {wrpc}.ByteWriter) (func({wrpc}.IndexWriter) error, error) {{
	    n := len(v)
	    if n > {math}.MaxUint32 {{
	        return nil, {fmt}.Errorf("list length of %d overflows a 32-bit integer", n)
	    }}
	    if err := func(v int, w {wrpc}.ByteWriter) error {{
	        b := make([]byte, {binary}.MaxVarintLen32)
	        i := {binary}.PutUvarint(b, uint64(v))
            {slog}.Debug("writing list length", "len", n)
	        _, err := w.Write(b[:i])
	        return err
        }}(n, w); err != nil {{
            return nil, {fmt}.Errorf("failed to write list length of %d: %w", n, err)
        }}
        {slog}.Debug("writing list elements")
	    writes := make(map[uint32]func({wrpc}.IndexWriter) error, n)
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
	    if len(writes) > 0 {{
	    	return func(w {wrpc}.IndexWriter) error {{
	    		var wg {errgroup}.Group
	    		for index, write := range writes {{
	    			w, err := w.Index(index)
	    			if err != nil {{
	    				return {fmt}.Errorf("failed to index writer: %w", err)
	    			}}
	    			write := write
	    			wg.Go(func() error {{
	    				return write(w)
	    			}})
	    		}}
	    		return wg.Wait()
	    	}}, nil
	    }}
	    return nil, nil
    }}({name}, {writer})"#
                );
            }

            TypeDefKind::Option(ty) => {
                let fmt = self.deps.fmt();
                let slog = self.deps.slog();
                let wrpc = self.deps.wrpc();

                self.push_str("func(v ");
                self.print_option(ty, true);
                uwrite!(
                    self.src,
                    r#", w {wrpc}.ByteWriter) (func({wrpc}.IndexWriter) error, error) {{
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

                let param = match ty {
                    Type::Id(id) => {
                        let ty = &self.resolve.types[*id];
                        match &ty.kind {
                            TypeDefKind::Enum(..) => "*v",
                            TypeDefKind::List(..) => "v",
                            _ => "*v",
                        }
                    }
                    _ => "*v",
                };
                self.print_write_ty(ty, param, "w");
                uwrite!(
                    self.src,
                    r#"
        if err != nil {{
		    return nil, {fmt}.Errorf("failed to write `option::some` payload: %w", err)
	    }}
        if write != nil {{
	    	return func(w {wrpc}.IndexWriter) error {{
	    		w, err := w.Index(1)
	    		if err != nil {{
	    			return {fmt}.Errorf("failed to index writer: %w", err)
	    		}}
                return write(w)
            }}, nil
        }}
	    return nil, nil
    }}({name}, {writer})"#
                );
            }

            TypeDefKind::Result(ty) => {
                let errors = self.deps.errors();
                let fmt = self.deps.fmt();
                let slog = self.deps.slog();
                let wrpc = self.deps.wrpc();

                self.push_str("func(v *");
                self.print_result(ty);
                uwriteln!(
                    self.src,
                    r#", w {wrpc}.ByteWriter) (func({wrpc}.IndexWriter) error, error) {{
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
                    self.print_write_ty(ty, "*v.Ok", "w");
                    uwriteln!(
                        self.src,
                        r#"
                    if err != nil {{
        			    return nil, {fmt}.Errorf("failed to write `result::ok` payload: %w", err)
        		    }}
                    if write != nil {{
	    	            return func(w {wrpc}.IndexWriter) error {{
	    		            w, err := w.Index(0)
	    		            if err != nil {{
	    		            	return {fmt}.Errorf("failed to index writer: %w", err)
	    		            }}
                            return write(w)
                        }}, nil
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
                    self.print_write_ty(ty, "*v.Err", "w");
                    uwriteln!(
                        self.src,
                        r#"
                if err != nil {{
        			return nil, {fmt}.Errorf("failed to write `result::err` payload: %w", err)
        		}}
                if write != nil {{
	    	        return func(w {wrpc}.IndexWriter) error {{
	    		        w, err := w.Index(1)
	    		        if err != nil {{
	    		        	return {fmt}.Errorf("failed to index writer: %w", err)
	    		        }}
                        return write(w)
                    }}, nil
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

            TypeDefKind::Variant(_) => panic!("unsupported anonymous variant"),

            TypeDefKind::Tuple(ty) => match ty.types.as_slice() {
                [] => self.push_str("error(nil), error(nil)"),
                [ty] => self.print_write_ty(ty, name, writer),
                _ => {
                    let fmt = self.deps.fmt();
                    let errgroup = self.deps.errgroup();
                    let wrpc = self.deps.wrpc();

                    self.push_str("func(v ");
                    self.print_tuple(ty, true);
                    uwriteln!(
                        self.src,
                        r", w {wrpc}.ByteWriter) (func({wrpc}.IndexWriter) error, error) {{
        writes := make(map[uint32]func({wrpc}.IndexWriter) error, {})",
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
                        r#"if len(writes) > 0 {{
	    	return func(w {wrpc}.IndexWriter) error {{
	    		var wg {errgroup}.Group
	    		for index, write := range writes {{
	    			w, err := w.Index(index)
	    			if err != nil {{
	    				return {fmt}.Errorf("failed to index writer: %w", err)
	    			}}
	    			write := write
	    			wg.Go(func() error {{
	    				return write(w)
	    			}})
	    		}}
	    		return wg.Wait()
	    	}}, nil
	    }}
        return nil, nil
    }}({name}, {writer})"#
                    );
                }
            },
            TypeDefKind::Resource => {
                panic!("unsupported anonymous type reference: resource")
            }
            TypeDefKind::Record(_) => {
                panic!("unsupported anonymous type reference: record")
            }
            TypeDefKind::Flags(_) => {
                panic!("unsupported anonymous type reference: flags")
            }
            TypeDefKind::Enum(_) => {
                panic!("unsupported anonymous type reference: enum")
            }
            TypeDefKind::Future(_ty) => uwrite!(
                self.src,
                r#"0, {errors}.New("writing futures not supported yet")"#,
                errors = self.deps.errors(),
            ),
            TypeDefKind::Stream(ty) => match ty.element {
                Some(Type::U8) => {
                    uwrite!(
                        self.src,
                        r#"func(v {wrpc}.ReadyReader, w {wrpc}.ByteWriter) (write func({wrpc}.IndexWriter) error, err error) {{
                if v.Ready() {{
		            defer func() {{
		            	body, ok := v.({io}.Closer)
		            	if ok {{
		            		if cErr := body.Close(); cErr != nil {{
		            			if err == nil {{
		            				err = {fmt}.Errorf("failed to close ready byte stream: %w", cErr)
		            			}} else {{
		            				slog.Warn("failed to close ready byte stream", "err", cErr)
		            			}}
		            		}}
		            	}}
		            }}()
		            {slog}.Debug("writing byte stream `stream::ready` status byte")
		            if err := w.WriteByte(1); err != nil {{
		            	return nil, {fmt}.Errorf("failed to write `stream::ready` byte: %w", err)
		            }}
		            {slog}.Debug("reading ready byte stream contents")
		            var buf {bytes}.Buffer
		            n, err := {io}.Copy(&buf, v)
		            if err != nil {{
		            	return nil, {fmt}.Errorf("failed to read ready byte stream contents: %w", err)
		            }}
		            {slog}.Debug("writing ready byte stream contents", "len", n)
		            if err = {wrpc}.WriteByteList(buf.Bytes(), w); err != nil {{
		            	return nil, {fmt}.Errorf("failed to write ready byte stream contents: %w", err)
		            }}
		            return nil, nil
                }} else {{
		            {slog}.Debug("writing byte stream `stream::pending` status byte")
		            if err := w.WriteByte(0); err != nil {{
		            	return nil, fmt.Errorf("failed to write `stream::pending` byte: %w", err)
		            }}
		            return func(w {wrpc}.IndexWriter) (err error) {{
		            	defer func() {{
		            		body, ok := v.({io}.Closer)
		            		if ok {{
		            			if cErr := body.Close(); cErr != nil {{
		            				if err == nil {{
		            					err = {fmt}.Errorf("failed to close pending byte stream: %w", cErr)
		            				}} else {{
		            					{slog}.Warn("failed to close pending byte stream", "err", cErr)
		            				}}
		            			}}
		            		}}
		            	}}()
		            	chunk := make([]byte, 8096)
		            	for {{
		            		var end bool
		            		{slog}.Debug("reading pending byte stream contents")
		            		n, err := v.Read(chunk)
		            		if err == {io}.EOF {{
		            			end = true
		            			{slog}.Debug("pending byte stream reached EOF")
		            		}} else if err != nil {{
		            			return {fmt}.Errorf("failed to read pending byte stream chunk: %w", err)
		            		}}
		            		if n > {math}.MaxUint32 {{
		            			return {fmt}.Errorf("pending byte stream chunk length of %d overflows a 32-bit integer", n)
		            		}}
		            		{slog}.Debug("writing pending byte stream chunk length", "len", n)
		            		if err := {wrpc}.WriteUint32(uint32(n), w); err != nil {{
		            			return {fmt}.Errorf("failed to write pending byte stream chunk length of %d: %w", n, err)
		            		}}
		            		_, err = w.Write(chunk[:n])
		            		if err != nil {{
		            			return {fmt}.Errorf("failed to write pending byte stream chunk contents: %w", err)
		            		}}
		            		if end {{
		            			if err := w.WriteByte(0); err != nil {{
		            				return {fmt}.Errorf("failed to write pending byte stream end byte: %w", err)
		            			}}
		            			return nil
		            		}}
		            	}}
		            }}, nil
                }}
            }}({name}, {writer})"#,
                        bytes = self.deps.bytes(),
                        fmt = self.deps.fmt(),
                        io = self.deps.io(),
                        math = self.deps.math(),
                        slog = self.deps.slog(),
                        wrpc = self.deps.wrpc(),
                    );
                }
                _ => uwrite!(
                    self.src,
                    r#"0, {errors}.New("writing non-byte streams not supported yet")"#,
                    errors = self.deps.errors(),
                ),
            },

            TypeDefKind::Handle(Handle::Own(_ty)) => uwrite!(
                self.src,
                r#"0, {errors}.New("writing owned handles not supported yet")"#,
                errors = self.deps.errors(),
            ),

            TypeDefKind::Handle(Handle::Borrow(_ty)) => uwrite!(
                self.src,
                r#"0, {errors}.New("writing borrowed handles not supported yet")"#,
                errors = self.deps.errors(),
            ),

            TypeDefKind::Type(ty) => self.print_write_ty(ty, name, writer),

            TypeDefKind::Unknown => unreachable!(),
        }
    }

    fn async_paths_ty(&mut self, ty: &Type) -> (Vec<VecDeque<Option<u32>>>, bool) {
        if let Type::Id(ty) = ty {
            self.async_paths_tyid(*ty)
        } else {
            (Vec::default(), false)
        }
    }

    fn async_paths_tyid(&mut self, id: TypeId) -> (Vec<VecDeque<Option<u32>>>, bool) {
        match &self.resolve.types[id].kind {
            TypeDefKind::List(ty) => {
                let mut paths = vec![];
                let (nested, fut) = self.async_paths_ty(ty);
                for mut path in nested {
                    path.push_front(None);
                    paths.push(path)
                }
                if fut {
                    paths.push(vec![None].into())
                }
                (paths, false)
            }
            TypeDefKind::Option(ty) => {
                let mut paths = vec![];
                let (nested, fut) = self.async_paths_ty(ty);
                for mut path in nested {
                    path.push_front(Some(1));
                    paths.push(path)
                }
                if fut {
                    paths.push(vec![Some(1)].into())
                }
                (paths, false)
            }
            TypeDefKind::Result(ty) => {
                let mut paths = vec![];
                if let Some(ty) = ty.ok.as_ref() {
                    let (nested, fut) = self.async_paths_ty(ty);
                    for mut path in nested {
                        path.push_front(Some(0));
                        paths.push(path)
                    }
                    if fut {
                        paths.push(vec![Some(0)].into())
                    }
                }
                if let Some(ty) = ty.err.as_ref() {
                    let (nested, fut) = self.async_paths_ty(ty);
                    for mut path in nested {
                        path.push_front(Some(1));
                        paths.push(path)
                    }
                    if fut {
                        paths.push(vec![Some(1)].into())
                    }
                }
                (paths, false)
            }
            TypeDefKind::Variant(ty) => {
                let mut paths = vec![];
                for (i, Case { ty, .. }) in ty.cases.iter().enumerate() {
                    if let Some(ty) = ty {
                        let (nested, fut) = self.async_paths_ty(ty);
                        for mut path in nested {
                            path.push_front(Some(i.try_into().unwrap()));
                            paths.push(path)
                        }
                        if fut {
                            paths.push(vec![Some(i.try_into().unwrap())].into())
                        }
                    }
                }
                (paths, false)
            }
            TypeDefKind::Tuple(ty) => {
                let mut paths = vec![];
                for (i, ty) in ty.types.iter().enumerate() {
                    let (nested, fut) = self.async_paths_ty(ty);
                    for mut path in nested {
                        path.push_front(Some(i.try_into().unwrap()));
                        paths.push(path)
                    }
                    if fut {
                        paths.push(vec![Some(i.try_into().unwrap())].into())
                    }
                }
                (paths, false)
            }
            TypeDefKind::Record(Record { fields }) => {
                let mut paths = vec![];
                for (i, Field { ty, .. }) in fields.iter().enumerate() {
                    let (nested, fut) = self.async_paths_ty(ty);
                    for mut path in nested {
                        path.push_front(Some(i.try_into().unwrap()));
                        paths.push(path)
                    }
                    if fut {
                        paths.push(vec![Some(i.try_into().unwrap())].into())
                    }
                }
                (paths, false)
            }
            TypeDefKind::Future(ty) => {
                let mut paths = vec![];
                if let Some(ty) = ty {
                    let (nested, fut) = self.async_paths_ty(ty);
                    for mut path in nested {
                        path.push_front(Some(0));
                        paths.push(path)
                    }
                    if fut {
                        paths.push(vec![Some(0)].into())
                    }
                }
                (paths, true)
            }
            TypeDefKind::Stream(Stream { element, .. }) => {
                let mut paths = vec![];
                if let Some(ty) = element {
                    let (nested, fut) = self.async_paths_ty(ty);
                    for mut path in nested {
                        path.push_front(None);
                        paths.push(path)
                    }
                    if fut {
                        paths.push(vec![None].into())
                    }
                }
                (paths, true)
            }
            TypeDefKind::Type(ty) => self.async_paths_ty(ty),
            TypeDefKind::Resource => (vec![], false),
            TypeDefKind::Flags(_) => (vec![], false),
            TypeDefKind::Enum(_) => (vec![], false),
            TypeDefKind::Handle(Handle::Own(_ty)) => (vec![], false),
            TypeDefKind::Handle(Handle::Borrow(_ty)) => (vec![], false),
            TypeDefKind::Unknown => unreachable!(),
        }
    }

    pub(super) fn generate_exports<'a>(
        &mut self,
        identifier: Identifier<'a>,
        funcs: impl Clone + ExactSizeIterator<Item = &'a Function>,
    ) -> bool {
        let mut traits = BTreeMap::new();
        let mut funcs_to_export = Vec::new();
        let mut resources_to_drop = Vec::new();

        traits.insert(None, ("Handler".to_string(), Vec::new()));

        if let Identifier::Interface(id, ..) = identifier {
            for (name, id) in &self.resolve.interfaces[id].types {
                match self.resolve.types[*id].kind {
                    TypeDefKind::Resource => {}
                    _ => continue,
                }
                resources_to_drop.push(name);
                let camel = name.to_upper_camel_case();
                traits.insert(Some(*id), (format!("Handler{camel}"), Vec::new()));
            }
        }

        let n = funcs.len();
        for func in funcs {
            if self.gen.skip.contains(&func.name) {
                continue;
            }

            let resource = match func.kind {
                FunctionKind::Freestanding => None,
                FunctionKind::Method(id)
                | FunctionKind::Constructor(id)
                | FunctionKind::Static(id) => Some(id),
            };
            funcs_to_export.push(func);
            let (_, methods) = traits.get_mut(&resource).unwrap();

            let prev = mem::take(&mut self.src);
            let sig = FnSig {
                use_item_name: true,
                ..Default::default()
            };
            self.print_docs_and_params(func, &sig);
            if let FunctionKind::Constructor(_) = &func.kind {
                uwriteln!(self.src, " ({}, error)", "Self"); // TODO: Use the correct Go name
            } else {
                self.src.push_str(" (");
                for (i, ty) in func.results.iter_types().enumerate() {
                    uwrite!(self.src, "r{i}__ ");
                    self.print_opt_ty(ty, true);
                    self.src.push_str(", ");
                }
                self.push_str("err__ error) ");
            }
            self.push_str("\n\n");
            let trait_method = mem::replace(&mut self.src, prev);
            methods.push(trait_method);
        }

        let (name, methods) = traits.remove(&None).unwrap();
        if !methods.is_empty() || !traits.is_empty() {
            self.generate_interface_trait(
                &name,
                &methods,
                traits.iter().map(|(resource, (trait_name, _methods))| {
                    (resource.unwrap(), trait_name.as_str())
                }),
            );
        }

        for (trait_name, methods) in traits.values() {
            uwriteln!(self.src, "type {trait_name} interface {{");
            for method in methods {
                self.src.push_str(method);
            }
            uwriteln!(self.src, "}}");
        }

        if !funcs_to_export
            .iter()
            .any(|Function { kind, .. }| matches!(kind, FunctionKind::Freestanding))
        {
            return false;
        }
        uwriteln!(
            self.src,
            "func ServeInterface(c {wrpc}.Client, h Handler) (stop func() error, err error) {{",
            wrpc = self.deps.wrpc(),
        );
        uwriteln!(self.src, r#"stops := make([]func() error, 0, {n})"#);
        self.src.push_str(
            r"stop = func() error {
            for _, stop := range stops {
                if err := stop(); err != nil {
                    return err
                }
            }
            return nil
        }
",
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
            Identifier::World(world) => {
                let World {
                    ref name, package, ..
                } = self.resolve.worlds[world];
                if let Some(package) = package {
                    self.resolve.id_of_name(package, name)
                } else {
                    name.to_string()
                }
            }
        };
        for (
            i,
            Function {
                kind,
                name,
                params,
                results,
                ..
            },
        ) in funcs_to_export.iter().enumerate()
        {
            if !matches!(kind, FunctionKind::Freestanding) {
                continue;
            }
            let bytes = self.deps.bytes();
            let errgroup = self.deps.errgroup();
            let context = self.deps.context();
            let fmt = self.deps.fmt();
            let slog = self.deps.slog();
            let wrpc = self.deps.wrpc();
            uwriteln!(
                self.src,
                r#"stop{i}, err := c.Serve("{instance}", "{name}", func(ctx {context}.Context, w {wrpc}.IndexWriter, r {wrpc}.IndexReadCloser) error {{"#,
            );
            for (i, (_, ty)) in params.iter().enumerate() {
                uwrite!(
                    self.src,
                    r#"{slog}.DebugContext(ctx, "reading parameter", "i", {i})
        p{i}, err := "#
                );
                self.print_read_ty(ty, "r", &i.to_string());
                self.push_str("\n");
                uwriteln!(
                    self.src,
                    r#"if err != nil {{ return {fmt}.Errorf("failed to read parameter {i}: %w", err) }}"#,
                );
            }
            uwriteln!(
                self.src,
                r#"{slog}.DebugContext(ctx, "calling `{instance}.{name}` handler")"#,
            );
            for (i, _) in results.iter_types().enumerate() {
                uwrite!(self.src, "r{i}, ");
            }
            self.push_str("err ");
            if results.len() > 0 {
                self.push_str(":");
            }
            self.push_str("= h.");
            self.push_str(&name.to_upper_camel_case());
            self.push_str("(ctx");
            for (i, _) in params.iter().enumerate() {
                uwrite!(self.src, ", p{i}");
            }
            self.push_str(")\n");
            self.push_str("if err != nil {\n");
            uwriteln!(
                self.src,
                r#"return {fmt}.Errorf("failed to handle `{instance}.{name}` invocation: %w", err)"#,
            );
            self.push_str("}\n");

            uwriteln!(
                self.src,
                r"
            var buf {bytes}.Buffer
            writes := make(map[uint32]func({wrpc}.IndexWriter) error, {})",
                results.len()
            );
            for (i, ty) in results.iter_types().enumerate() {
                uwrite!(self.src, "write{i}, err :=");
                self.print_write_ty(ty, &format!("r{i}"), "&buf");
                self.push_str("\n");
                self.push_str("if err != nil {\n");
                uwriteln!(
                    self.src,
                    r#"return {fmt}.Errorf("failed to write result value {i}: %w", err)"#,
                );
                self.src.push_str("}\n");
                uwriteln!(
                    self.src,
                    r#"if write{i} != nil {{
                    writes[{i}] = write{i}
                }}"#,
                );
            }
            uwrite!(
                self.src,
                r#"{slog}.DebugContext(ctx, "transmitting `{instance}.{name}` result")
                _, err = w.Write(buf.Bytes())
                if err != nil {{
                    return {fmt}.Errorf("failed to write result: %w", err)
                }}
                if len(writes) > 0 {{
	            	var wg {errgroup}.Group
	            	for index, write := range writes {{
	            		w, err := w.Index(index)
	            		if err != nil {{
	            			return {fmt}.Errorf("failed to index writer: %w", err)
	            		}}
	            		write := write
	            		wg.Go(func() error {{
	            			return write(w)
	            		}})
	            	}}
	            	return wg.Wait()
	            }}
                return nil
             }}"#,
            );
            for (i, (_, ty)) in params.iter().enumerate() {
                let (nested, fut) = self.async_paths_ty(ty);
                for path in nested {
                    self.push_str(wrpc);
                    self.push_str("NewSubscribePath().Index(");
                    uwrite!(self.src, "{i})");
                    for p in path {
                        if let Some(p) = p {
                            uwrite!(self.src, ".Index({p})")
                        } else {
                            self.push_str(".Wildcard()")
                        }
                    }
                    self.push_str(", ")
                }
                if fut {
                    uwrite!(self.src, "{wrpc}.NewSubscribePath().Index({i})");
                }
            }
            uwriteln!(
                self.src,
                r#")
             if err != nil {{
                 return nil, {fmt}.Errorf("failed to serve `{instance}.{name}`: %w", err)
             }}
             stops = append(stops, stop{i})"#,
            );
        }
        self.push_str("return stop, nil\n");
        self.push_str("}\n");
        true
    }

    fn generate_interface_trait<'a>(
        &mut self,
        trait_name: &str,
        methods: &[Source],
        resource_traits: impl Iterator<Item = (TypeId, &'a str)>,
    ) {
        uwriteln!(self.src, "type {trait_name} interface {{");
        for (id, trait_name) in resource_traits {
            let name = self.resolve.types[id]
                .name
                .as_ref()
                .unwrap()
                .to_upper_camel_case();
            uwriteln!(self.src, "//type {name} {trait_name}");
        }
        for method in methods {
            self.src.push_str(method);
        }
        uwriteln!(self.src, "}}");
    }

    pub fn generate_imports<'a>(
        &mut self,
        instance: &str,
        funcs: impl Iterator<Item = &'a Function>,
    ) {
        for func in funcs {
            self.generate_guest_import(instance, func);
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
            &mut self.gen.import_modules
        } else {
            &mut self.gen.export_modules
        };
        map.push((module, module_path));
    }

    fn generate_guest_import(&mut self, instance: &str, func: &Function) {
        if self.gen.skip.contains(&func.name) {
            return;
        }

        let sig = FnSig::default();
        match func.kind {
            FunctionKind::Freestanding => {}
            FunctionKind::Method(_id)
            | FunctionKind::Static(_id)
            | FunctionKind::Constructor(_id) => {
                return;
                // TODO: Generate
                //let name = self.resolve.types[id].name.as_ref().unwrap();
                //let name = to_upper_camel_case(name);
                //sig.use_item_name = true;
                //if let FunctionKind::Method(_) = &func.kind {
                //    sig.self_arg = Some(format!("self__ {name}"));
                //    sig.self_is_first_param = true;
                //}
            }
        }
        let fmt = self.deps.fmt();
        let wrpc = self.deps.wrpc();

        let _params = self.print_docs_and_params(func, &sig);
        if let FunctionKind::Constructor(_) = &func.kind {
            uwrite!(self.src, " (Self, error)");
        } else {
            self.src.push_str(" (");
            for (i, ty) in func.results.iter_types().enumerate() {
                uwrite!(self.src, "r{i}__ ");
                self.print_opt_ty(ty, true);
                self.src.push_str(", ");
            }
            self.push_str("close__ func() error, err__ error) ");
        }
        self.src.push_str("{\n");
        self.src.push_str("if err__ = wrpc__.Invoke(ctx__, ");
        match func.kind {
            FunctionKind::Freestanding
            | FunctionKind::Static(..)
            | FunctionKind::Constructor(..) => {
                uwrite!(self.src, r#""{instance}""#);
            }
            FunctionKind::Method(..) => {
                self.src.push_str("self.0");
            }
        }
        self.src.push_str(", \"");
        self.src.push_str(&func.name);
        self.src.push_str("\", ");
        uwriteln!(
            self.src,
            "func(w__ {wrpc}.IndexWriter, r__ {wrpc}.IndexReadCloser) error {{"
        );
        self.push_str("close__ = r__.Close\n");
        if !func.params.is_empty() {
            let bytes = self.deps.bytes();
            uwriteln!(
                self.src,
                r"var buf__ {bytes}.Buffer
        writes__ := make(map[uint32]func({wrpc}.IndexWriter) error, {})",
                func.params.len(),
            );
            for (i, (name, ty)) in func.params.iter().enumerate() {
                uwrite!(self.src, "write{i}__, err__ :=");
                self.print_write_ty(ty, &to_go_ident(name), "&buf__");
                self.src.push_str("\nif err__ != nil {\n");
                uwriteln!(
                    self.src,
                    r#"return {fmt}.Errorf("failed to write `{name}` parameter: %w", err__)"#,
                );
                self.src.push_str("}\n");
                uwriteln!(
                    self.src,
                    r#"if write{i}__ != nil {{
                writes__[{i}] = write{i}__
        }}"#,
                );
            }
            self.push_str("_, err__ = w__.Write(buf__.Bytes())\n");
            self.push_str("if err__ != nil {\n");
            uwriteln!(
                self.src,
                r#"return {fmt}.Errorf("failed to write parameters: %w", err__)"#,
            );
            self.src.push_str("}\n");
        } else {
            self.push_str("_, err__ = w__.Write(nil)\n");
            self.push_str("if err__ != nil {\n");
            uwriteln!(
                self.src,
                r#"return {fmt}.Errorf("failed to write empty parameters: %w", err__)"#,
            );
            self.src.push_str("}\n");
        }
        for (i, ty) in func.results.iter_types().enumerate() {
            uwrite!(self.src, "r{i}__, err__ = ");
            self.print_read_ty(ty, "r__", &i.to_string());
            self.push_str("\n");
            uwriteln!(
                self.src,
                r#"if err__ != nil {{ return {fmt}.Errorf("failed to read result {i}: %w", err__) }}"#,
            );
        }
        self.src.push_str("return nil\n");
        self.src.push_str("},");
        for (i, ty) in func.results.iter_types().enumerate() {
            let (nested, fut) = self.async_paths_ty(ty);
            for path in nested {
                self.push_str(wrpc);
                self.push_str("NewSubscribePath().Index(");
                uwrite!(self.src, "{i})");
                for p in path {
                    if let Some(p) = p {
                        uwrite!(self.src, ".Index({p})")
                    } else {
                        self.push_str(".Wildcard()")
                    }
                }
                self.push_str(", ")
            }
            if fut {
                uwrite!(self.src, "{wrpc}.NewSubscribePath().Index({i})");
            }
        }
        self.src.push_str("); err__ != nil {\n");
        uwriteln!(
            self.src,
            r#"err__ = {fmt}.Errorf("failed to invoke `{}`: %w", err__)
            return
        }}
        return
    }}"#,
            func.name
        );
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

    fn godoc_params(&mut self, docs: &[(String, Type)], header: &str) {
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

    fn print_docs_and_params(&mut self, func: &Function, sig: &FnSig) -> Vec<String> {
        self.godoc(&func.docs);
        self.godoc_params(&func.params, "Parameters");
        // TODO: re-add this when docs are back
        // self.godoc_params(&func.results, "Return");

        if self.in_import {
            self.push_str("func ");
        }
        let func_name = if sig.use_item_name {
            if let FunctionKind::Constructor(_) = &func.kind {
                "New"
            } else {
                func.item_name()
            }
        } else {
            &func.name
        };
        if let Some(arg) = &sig.self_arg {
            self.push_str("(");
            self.push_str(arg);
            self.push_str(")");
        }
        self.push_str(&func_name.to_upper_camel_case());
        let context = self.deps.context();
        uwrite!(self.src, "(ctx__ {context}.Context, ");
        if self.in_import {
            let wrpc = self.deps.wrpc();
            uwrite!(self.src, "wrpc__ {wrpc}.Client, ");
        }
        let mut params = Vec::new();
        for (i, (name, param)) in func.params.iter().enumerate() {
            if let FunctionKind::Method(..) = &func.kind {
                if i == 0 && sig.self_is_first_param {
                    params.push("self".to_string());
                    continue;
                }
            }
            let name = to_go_ident(name);
            self.push_str(&name);
            self.push_str(" ");
            self.print_opt_ty(param, true);
            self.push_str(",");

            params.push(name);
        }
        self.push_str(")");
        params
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
        }
    }

    fn print_optional_ty(&mut self, ty: Option<&Type>) {
        match ty {
            Some(ty) => self.print_ty(ty, true),
            None => self.push_str("struct{}"),
        }
    }

    fn type_path_with_name(&mut self, id: TypeId, name: String) -> String {
        if let TypeOwner::Interface(id) = self.resolve.types[id].owner {
            if let Some(path) = self.path_to_interface(id) {
                return format!("{path}.{name}");
            }
        }
        name
    }

    fn result_element_ptr(&mut self, ty: &Type, decl: bool) {
        if let Type::Id(id) = ty {
            match &self.resolve.types[*id].kind {
                TypeDefKind::Option(..) | TypeDefKind::List(..) | TypeDefKind::Enum(..) => {}
                TypeDefKind::Tuple(Tuple { types }) if types.len() == 1 => {
                    self.result_element_ptr(&types[0], decl);
                    return;
                }
                TypeDefKind::Type(ty) => {
                    self.result_element_ptr(ty, decl);
                    return;
                }
                _ => return,
            }
        }
        if decl {
            self.push_str("*");
        } else {
            self.push_str("&");
        }
    }

    fn print_tuple(&mut self, Tuple { types }: &Tuple, decl: bool) {
        match types.as_slice() {
            [] => self.push_str("struct{}"),
            [ty] => self.print_opt_ty(ty, decl),
            _ => {
                let wrpc = self.deps.wrpc();
                if decl {
                    self.push_str("*");
                } else {
                    self.push_str("&");
                }
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

    fn print_opt_ty(&mut self, ty: &Type, decl: bool) {
        match ty {
            Type::Id(id) => {
                let ty = &self.resolve.types[*id];
                match &ty.kind {
                    TypeDefKind::Enum(..) => {
                        let name = ty.name.as_ref().expect("enum missing a name");
                        let name = self.type_path_with_name(*id, to_upper_camel_case(name));
                        self.push_str(&name);
                    }
                    TypeDefKind::List(ty) => self.print_list(ty),
                    TypeDefKind::Option(ty) => self.print_option(ty, decl),
                    TypeDefKind::Tuple(ty) => self.print_tuple(ty, decl),
                    TypeDefKind::Future(ty) => self.print_future(ty),
                    TypeDefKind::Stream(ty) => self.print_stream(ty),
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
        }
    }

    fn print_option(&mut self, ty: &Type, decl: bool) {
        if let Type::Id(id) = ty {
            if let TypeDefKind::List(t) = self.resolve.types[*id].kind {
                // Go slices are pointer types
                self.print_list(&t);
                return;
            }
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

    fn print_future(&mut self, ty: &Option<Type>) {
        let wrpc = self.deps.wrpc();
        self.push_str(wrpc);
        self.push_str(".ReadyReceiver[");
        self.print_optional_ty(ty.as_ref());
        self.push_str("]");
    }

    fn print_stream(&mut self, ty: &Stream) {
        let wrpc = self.deps.wrpc();
        self.push_str(wrpc);
        match ty.element {
            Some(Type::U8) => {
                self.push_str(".ReadyReader");
            }
            _ => {
                self.push_str(".ReadyReceiver[[]");
                self.print_optional_ty(ty.element.as_ref());
                self.push_str("]");
            }
        }
    }

    fn print_tyid(&mut self, id: TypeId, decl: bool) {
        let ty = &self.resolve.types[id];
        if let Some(name) = &ty.name {
            let name = self.type_path_with_name(id, to_upper_camel_case(name));
            self.push_str(&name);
            return;
        }

        match &ty.kind {
            TypeDefKind::List(t) => self.print_list(t),
            TypeDefKind::Option(t) => self.print_option(t, decl),
            TypeDefKind::Result(r) => self.print_result(r),

            TypeDefKind::Variant(_) => panic!("unsupported anonymous variant"),

            // Tuple-like records are mapped directly to wrpc tuples of
            // types.
            TypeDefKind::Tuple(t) => self.print_tuple(t, decl),
            TypeDefKind::Resource => {
                panic!("unsupported anonymous type reference: resource")
            }
            TypeDefKind::Record(_) => {
                panic!("unsupported anonymous type reference: record")
            }
            TypeDefKind::Flags(_) => {
                panic!("unsupported anonymous type reference: flags")
            }
            TypeDefKind::Enum(_) => {
                panic!("unsupported anonymous type reference: enum")
            }
            TypeDefKind::Future(ty) => self.print_future(ty),
            TypeDefKind::Stream(ty) => self.print_stream(ty),

            TypeDefKind::Handle(Handle::Own(ty)) => {
                self.print_ty(&Type::Id(*ty), decl);
            }

            TypeDefKind::Handle(Handle::Borrow(ty)) => {
                if self.is_exported_resource(*ty) {
                    let camel = self.resolve.types[*ty]
                        .name
                        .as_deref()
                        .unwrap()
                        .to_upper_camel_case();
                    let name = self.type_path_with_name(*ty, format!("{camel}Borrow"));
                    self.push_str(&name);
                } else {
                    let ty = &Type::Id(*ty);
                    self.print_ty(ty, decl);
                }
            }

            TypeDefKind::Type(t) => self.print_ty(t, decl),

            TypeDefKind::Unknown => unreachable!(),
        }
    }

    fn print_list(&mut self, ty: &Type) {
        self.push_str("[]");
        self.print_opt_ty(ty, true);
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
        } = &self.gen.interface_names[&interface];
        if let Identifier::Interface(cur, _) = self.identifier {
            if cur == interface {
                return None;
            }
        }
        Some(self.deps.import(import_name.clone(), import_path.clone()))
    }

    pub fn is_exported_resource(&self, ty: TypeId) -> bool {
        let ty = dealias(self.resolve, ty);
        let ty = &self.resolve.types[ty];
        match &ty.kind {
            TypeDefKind::Resource => {}
            _ => return false,
        }

        match ty.owner {
            // Worlds cannot export types of any kind as of this writing.
            TypeOwner::World(_) => false,

            // Interfaces are "stateful" currently where whatever we last saw
            // them as dictates whether it's exported or not.
            TypeOwner::Interface(i) => !self.gen.interface_last_seen_as_import[&i],

            // Shouldn't be the case for resources
            TypeOwner::None => unreachable!(),
        }
    }

    fn push_str(&mut self, s: &str) {
        self.src.push_str(s);
    }

    fn info(&self, ty: TypeId) -> TypeInfo {
        self.gen.types.get(ty)
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
            for Field { name, ty, docs } in fields {
                self.godoc(docs);
                self.push_str(&name.to_upper_camel_case());
                self.push_str(" ");
                self.print_opt_ty(ty, true);
                self.push_str("\n");
            }
            self.push_str("}\n");

            let wrpc = self.deps.wrpc();

            // TODO: Print something more useful
            uwriteln!(
                self.src,
                r#"func (v *{name}) String() string {{ return "{name}" }}

func (v *{name}) WriteToIndex(w {wrpc}.ByteWriter) (func({wrpc}.IndexWriter) error, error) {{
    writes := make(map[uint32]func({wrpc}.IndexWriter) error, {})"#,
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
            let fmt = self.deps.fmt();
            let errgroup = self.deps.errgroup();
            uwriteln!(
                self.src,
                r#"
    if len(writes) > 0 {{
    	return func(w {wrpc}.IndexWriter) error {{
    		var wg {errgroup}.Group
    		for index, write := range writes {{
    			w, err := w.Index(index)
    			if err != nil {{
    				return {fmt}.Errorf("failed to index writer: %w", err)
    			}}
    			write := write
    			wg.Go(func() error {{
    				return write(w)
    			}})
    		}}
    		return wg.Wait()
    	}}, nil
    }}
    return nil, nil
}}"#
            );

            uwriteln!(
                self.src,
                r#"func Read{name}(r {wrpc}.ByteReader) (*{name}, error) {{
    v := &{name}{{}}
    var err error"#
            );
            for (i, Field { name, ty, .. }) in fields.iter().enumerate() {
                let fmt = self.deps.fmt();
                let slog = self.deps.slog();
                let ident = name.to_upper_camel_case();
                uwrite!(
                    self.src,
                    r#"{slog}.Debug("reading field", "name", "{name}")
    v.{ident}, err = "#
                );
                self.print_read_ty(ty, "r", &i.to_string());
                self.push_str("\n");
                uwriteln!(
                    self.src,
                    r#"if err != nil {{
		    return nil, {fmt}.Errorf("failed to read `{name}` field: %w", err)
	    }}"#
                );
            }
            self.push_str("return v, nil\n");
            self.push_str("}\n");

            if info.error {
                uwriteln!(
                    self.src,
                    r#"func (v *{name}) Error() string {{ return v.String() }}"#
                );
            }
        }
    }

    fn type_resource(&mut self, id: TypeId, _name: &str, docs: &Docs) {
        // TODO: Support resources
        if let Some(name) = self.name_of(id) {
            self.godoc(docs);
            self.push_str(&format!("type {name}"));
            self.push_str(" = struct{}\n");
        }
    }

    fn type_tuple(&mut self, id: TypeId, _name: &str, tuple: &Tuple, docs: &Docs) {
        if let Some(name) = self.name_of(id) {
            self.godoc(docs);
            self.push_str(&format!("type {name}"));
            self.push_str(" = ");
            self.print_tuple(tuple, true);
            self.push_str("\n");
        }
    }

    fn type_flags(&mut self, id: TypeId, _name: &str, Flags { flags }: &Flags, docs: &Docs) {
        let repr = match flags.len() {
            ..=8 => Int::U8,
            9..=16 => Int::U16,
            17..=32 => Int::U32,
            33.. => Int::U64,
        };

        let info = self.info(id);
        if let Some(name) = self.name_of(id) {
            let fmt = self.deps.fmt();
            let strings = self.deps.strings();
            let wrpc = self.deps.wrpc();

            self.godoc(docs);
            uwriteln!(self.src, "type {name} struct {{");
            for Flag { name, docs } in flags {
                self.godoc(docs);
                self.push_str(&name.to_upper_camel_case());
                self.push_str(" bool\n");
            }
            self.push_str("}\n");

            uwriteln!(self.src, "func (v *{name}) String() string {{");
            uwriteln!(self.src, "flags := make([]string, 0, {})", flags.len());
            for Flag { name, .. } in flags {
                self.push_str("if v.");
                self.push_str(&name.to_upper_camel_case());
                self.push_str(" {\n");
                uwriteln!(self.src, r#"flags = append(flags, "{name}")"#);
                self.push_str("}\n");
            }
            uwriteln!(self.src, r#"return {strings}.Join(flags, " | ")"#);
            self.push_str("}\n");
            uwriteln!(
                self.src,
                "func (v *{name}) WriteToIndex(w {wrpc}.ByteWriter) (func({wrpc}.IndexWriter) error, error) {{"
            );
            self.push_str("var n ");
            self.int_repr(repr);
            self.push_str("\n");
            for (i, Flag { name, .. }) in flags.iter().enumerate() {
                self.push_str("if v.");
                self.push_str(&name.to_upper_camel_case());
                self.push_str(" {\n");
                if i <= 64 {
                    uwriteln!(self.src, "n |= 1 << {i}");
                } else {
                    let errors = self.deps.errors();
                    uwriteln!(
                        self.src,
                        r#"return nil, {errors}.New("encoding `{name}` flag value would overflow 64-bit integer, flags containing more than 64 members are not supported yet")"#
                    );
                }
                self.push_str("}\n");
            }
            self.push_str("return nil, ");
            self.print_write_discriminant(repr, "n", "w");
            self.push_str("\n");
            self.push_str("}\n");

            uwrite!(
                self.src,
                r#"func Read{name}(r {wrpc}.ByteReader) (*{name}, error) {{
    v := &{name}{{}}
    n, err := "#
            );
            self.print_read_discriminant(repr, "r");
            self.push_str("\n");
            self.push_str("if err != nil {\n");
            self.push_str("return nil, ");
            self.push_str(fmt);
            self.push_str(".Errorf(\"failed to read flag: %w\", err)\n");
            self.push_str("}\n");
            for (i, Flag { name, .. }) in flags.iter().enumerate() {
                if i > 64 {
                    break;
                }
                uwriteln!(self.src, "if n & (1 << {i}) > 0 {{");
                self.push_str("v.");
                self.push_str(&name.to_upper_camel_case());
                self.push_str(" = true\n");
                self.push_str("}\n");
            }
            self.push_str("return v, nil\n");
            self.push_str("}\n");

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
                self.push_str("Discriminant_");
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
                self.push_str("Discriminant_");
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
                    r#"if ok = (v.discriminant == {name}Discriminant_{camel}); !ok {{ return }}"#
                );
                if let Some(ty) = ty {
                    self.push_str("payload, ok = v.payload.(");
                    self.print_ty(ty, true);
                    self.push_str(")\n");
                }
                self.push_str("return\n}\n");

                self.godoc(docs);
                uwrite!(self.src, r#"func New{name}_{camel}("#);
                if let Some(ty) = ty {
                    self.push_str("payload ");
                    self.print_opt_ty(ty, true);
                }
                uwriteln!(self.src, ") *{name} {{ return &{name}{{");
                if ty.is_some() {
                    self.push_str("payload");
                } else {
                    self.push_str("nil");
                }
                uwriteln!(self.src, r#", {name}Discriminant_{camel} }} }}"#);
            }

            if info.error {
                uwriteln!(
                    self.src,
                    r#"func (v *{name}) Error() string {{ return v.String() }}"#
                );
            }

            let fmt = self.deps.fmt();
            let wrpc = self.deps.wrpc();
            uwriteln!(
                self.src,
                "func (v *{name}) WriteToIndex(w {wrpc}.ByteWriter) (func({wrpc}.IndexWriter) error, error) {{",
            );
            self.push_str("if err := ");
            self.print_write_discriminant(variant.tag(), "v.discriminant", "w");
            self.push_str("; err != nil { return nil, ");
            self.push_str(fmt);
            self.push_str(".Errorf(\"failed to write discriminant: %w\", err)\n}\n");
            let errors = self.deps.errors();
            self.push_str("switch v.discriminant {\n");
            for (
                i,
                Case {
                    name: case_name,
                    ty,
                    ..
                },
            ) in variant.cases.iter().enumerate()
            {
                self.push_str("case ");
                self.push_str(&name);
                self.push_str("Discriminant_");
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
                    uwriteln!(
                        self.src,
                        r#"
                    if write != nil {{
	                	return func(w {wrpc}.IndexWriter) error {{
	                		w, err := w.Index({i})
	                		if err != nil {{
	                			return {fmt}.Errorf("failed to index writer: %w", err)
	                		}}
                            return write(w)
                        }}, nil
                    }} "#
                    )
                }
            }
            self.push_str("default: return nil, ");
            self.push_str(errors);
            self.push_str(".New(\"invalid variant\")\n}\n");
            self.push_str("return nil, nil\n");
            self.push_str("}\n");

            uwrite!(
                self.src,
                r#"func Read{name}(r {wrpc}.ByteReader) (*{name}, error) {{
    disc, err := "#,
            );
            self.print_read_discriminant(variant.tag(), "r");
            self.push_str("\n");
            self.push_str("if err != nil {\n");
            self.push_str("return nil, ");
            self.push_str(fmt);
            self.push_str(".Errorf(\"failed to read discriminant: %w\", err)\n}\n");
            uwriteln!(self.src, "switch {name}Discriminant(disc) {{");
            for (
                i,
                Case {
                    name: case_name,
                    ty,
                    ..
                },
            ) in variant.cases.iter().enumerate()
            {
                self.push_str("case ");
                self.push_str(&name);
                self.push_str("Discriminant_");
                self.push_str(&case_name.to_upper_camel_case());
                self.push_str(":\n");
                if let Some(ty) = ty {
                    self.push_str("payload, err := ");
                    self.print_read_ty(ty, "r", &i.to_string());
                    self.push_str("\n");
                    self.push_str("if err != nil { return nil, ");
                    self.push_str(fmt);
                    uwriteln!(
                        self.src,
                        r#".Errorf("failed to read `{case_name}` payload: %w", err) }}"#
                    );
                    uwriteln!(
                        self.src,
                        "return New{name}_{}(payload), nil",
                        case_name.to_upper_camel_case()
                    );
                } else {
                    uwriteln!(
                        self.src,
                        "return New{name}_{}(), nil",
                        case_name.to_upper_camel_case()
                    );
                }
            }
            uwriteln!(
                self.src,
                r#"default: return nil, {fmt}.Errorf("unknown discriminant value %d", disc) }}"#
            );
            self.push_str("}\n");
        }
    }

    fn type_option(&mut self, id: TypeId, _name: &str, payload: &Type, docs: &Docs) {
        if let Some(name) = self.name_of(id) {
            self.godoc(docs);
            self.push_str(&format!("type {name}"));
            self.push_str("=");
            self.print_option(payload, true);
            self.push_str("\n");
        }
    }

    fn type_result(&mut self, id: TypeId, _name: &str, result: &Result_, docs: &Docs) {
        if let Some(name) = self.name_of(id) {
            self.godoc(docs);
            self.push_str(&format!("type {name}"));
            self.push_str("=");
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

            let fmt = self.deps.fmt();
            let wrpc = self.deps.wrpc();
            uwriteln!(
                self.src,
                "func (v {name}) WriteToIndex(w {wrpc}.ByteWriter) (func({wrpc}.IndexWriter) error, error) {{",
            );
            self.push_str("if err := ");
            self.print_write_discriminant(enum_.tag(), "v", "w");
            self.push_str("; err != nil { return nil, ");
            self.push_str(fmt);
            self.push_str(".Errorf(\"failed to write discriminant: %w\", err)\n}\n");
            self.push_str("return nil, nil\n");
            self.push_str("}\n");

            uwrite!(
                self.src,
                r#"func Read{name}(r {wrpc}.ByteReader) (v {name}, err error) {{
    disc, err := "#,
            );
            self.print_read_discriminant(enum_.tag(), "r");
            self.push_str("\n");
            self.push_str("if err != nil {\n");
            self.push_str("return v, ");
            self.push_str(fmt);
            self.push_str(".Errorf(\"failed to read discriminant: %w\", err)\n}\n");
            uwriteln!(self.src, "switch {name}(disc) {{");
            for EnumCase {
                name: case_name, ..
            } in &enum_.cases
            {
                self.push_str("case ");
                self.push_str(&name);
                self.push_str("_");
                self.push_str(&case_name.to_upper_camel_case());
                self.push_str(":\n");
                self.push_str("return ");
                self.push_str(&name);
                self.push_str("_");
                self.push_str(&case_name.to_upper_camel_case());
                self.push_str(", nil\n");
            }
            uwriteln!(
                self.src,
                r#"default: return v, {fmt}.Errorf("unknown discriminant value %d", disc) }}"#
            );
            self.push_str("}\n");
        }
    }

    fn type_alias(&mut self, id: TypeId, _name: &str, ty: &Type, docs: &Docs) {
        if let Some(name) = self.name_of(id) {
            self.godoc(docs);
            self.push_str(&format!("type {name}"));
            self.push_str(" = ");
            self.print_ty(ty, true);
            self.push_str("\n");
        }

        if self.is_exported_resource(id) {
            self.godoc(docs);
            let name = self.resolve.types[id].name.as_ref().unwrap();
            let name = name.to_upper_camel_case();
            self.push_str(&format!("type {name}Borrow"));
            self.push_str(" = ");
            self.print_ty(ty, true);
            self.push_str("Borrow");
            self.push_str("\n");
        }
    }

    fn type_list(&mut self, id: TypeId, _name: &str, ty: &Type, docs: &Docs) {
        if let Some(name) = self.name_of(id) {
            self.godoc(docs);
            self.push_str(&format!("type {name}"));
            self.push_str(" = ");
            self.print_list(ty);
            self.push_str("\n");
        }
    }

    fn type_builtin(&mut self, _id: TypeId, name: &str, ty: &Type, docs: &Docs) {
        self.godoc(docs);
        self.src
            .push_str(&format!("type {}", name.to_upper_camel_case()));
        self.src.push_str(" = ");
        self.print_ty(ty, true);
        self.src.push_str("\n");
    }
}
