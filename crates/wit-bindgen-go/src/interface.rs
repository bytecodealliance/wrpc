use crate::{
    int_repr, to_go_ident, to_package_ident, to_upper_camel_case, Deps, FnSig, GoWrpc, Identifier,
    InterfaceName, RustFlagsRepr,
};
use heck::{ToShoutySnakeCase, ToUpperCamelCase};
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::mem;
use wit_bindgen_core::{
    dealias, uwrite, uwriteln,
    wit_parser::{
        Case, Docs, Enum, EnumCase, Field, Flags, Function, FunctionKind, Handle, Int, InterfaceId,
        Record, Resolve, Result_, Results, Tuple, Type, TypeDefKind, TypeId, TypeOwner, Variant,
        World, WorldKey,
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
    fn print_read_ty(&mut self, ty: &Type, reader: &str) {
        // NOTE: LEB128 decoding adapted from
        // https://cs.opensource.google/go/go/+/refs/tags/go1.22.2:src/encoding/binary/varint.go;l=128-153
        match ty {
            Type::Id(t) => self.print_read_tyid(*t, reader),
            Type::Bool => uwrite!(
                self.src,
                r#"func(r {wrpc}.ByteReader) (bool, error) {{
    {slog}.Debug("reading `bool` byte")
    v, err := r.ReadByte()
    if err != nil {{
        return false, {fmt}.Errorf("failed to read `bool` byte: %w", err)
    }}
    return v == 1, nil
}}({reader})"#,
                fmt = self.deps.fmt(),
                slog = self.deps.slog(),
                wrpc = self.deps.wrpc(),
            ),
            Type::U8 => uwrite!(
                self.src,
                r#"func(r {wrpc}.ByteReader) (uint8, error) {{
    {slog}.Debug("reading `u8` byte")
    v, err := r.ReadByte()
    if err != nil {{
        return 0, {fmt}.Errorf("failed to read `u8` byte: %w", err)
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
        {slog}.Debug("reading `uint16` byte", "i", i)
		b, err := r.ReadByte()
		if err != nil {{
			if i > 0 && err == {io}.EOF {{
				err = {io}.ErrUnexpectedEOF
			}}
			return x, {fmt}.Errorf("failed to read `uint16` byte: %w", err)
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
        {slog}.Debug("reading `uint32` byte", "i", i)
		b, err := r.ReadByte()
		if err != nil {{
			if i > 0 && err == {io}.EOF {{
				err = {io}.ErrUnexpectedEOF
			}}
			return x, {fmt}.Errorf("failed to read `uint32` byte: %w", err)
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
        {slog}.Debug("reading `uint64` byte", "i", i)
		b, err := r.ReadByte()
		if err != nil {{
			if i > 0 && err == {io}.EOF {{
				err = {io}.ErrUnexpectedEOF
			}}
			return x, {fmt}.Errorf("failed to read `uint64` byte: %w", err)
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
                r#"int8(0), {errors}.New("reading s8 not supported yet")"#,
                errors = self.deps.errors(),
            ),
            Type::S16 => uwrite!(
                self.src,
                r#"int16(0), {errors}.New("reading s16 not supported yet")"#,
                errors = self.deps.errors(),
            ),
            Type::S32 => uwrite!(
                self.src,
                r#"int32(0), {errors}.New("reading s32 not supported yet")"#,
                errors = self.deps.errors(),
            ),
            Type::S64 => uwrite!(
                self.src,
                r#"int64(0), {errors}.New("reading s64 not supported yet")"#,
                errors = self.deps.errors(),
            ),
            Type::F32 => uwrite!(
                self.src,
                r#"func(r {wrpc}.ByteReader) (float32, error) {{
    var b [4]byte
    {slog}.Debug("reading `float32` bytes")
    _, err := r.Read(b[:])
    if err != nil {{
        return 0, {fmt}.Errorf("failed to read `float32`: %w", err)
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
    {slog}.Debug("reading `float64` bytes")
    _, err := r.Read(b[:])
    if err != nil {{
        return 0, {fmt}.Errorf("failed to read `float64`: %w", err)
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

    fn print_read_tyid(&mut self, id: TypeId, reader: &str) {
        let ty = &self.resolve.types[id];
        if let Some(ref name) = ty.name {
            let read = self.type_path_with_name(id, format!("Read{}", to_upper_camel_case(name)));
            uwrite!(self.src, "{read}({reader})",);
            return;
        }

        match &ty.kind {
            TypeDefKind::List(ty) => {
                let fmt = self.deps.fmt();
                let io = self.deps.io();
                let errors = self.deps.errors();
                let slog = self.deps.slog();
                let wrpc = self.deps.wrpc();
                uwrite!(self.src, "func(r {wrpc}.ByteReader) (",);
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
                self.print_read_ty(ty, "r");
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
                uwrite!(self.src, "func(r {wrpc}.ByteReader) (",);
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
                self.print_read_ty(ty, "r");
                self.push_str("\n");
                uwrite!(
                    self.src,
                    r#"if err != nil {{
	    	return nil, {fmt}.Errorf("failed to read `option::some` value: %w", err)
	    }}
	    return &v, nil
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
                uwrite!(self.src, "func(r {wrpc}.ByteReader) (*",);
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
                    self.print_read_ty(ty, "r");
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
                    self.print_read_ty(err, "r");
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
                [ty] => self.print_read_ty(ty, reader),
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
                        self.print_read_ty(ty, "r");
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
                r#"0, {errors}.New("reading futures not supported yet")"#,
                errors = self.deps.errors(),
            ),
            TypeDefKind::Stream(_stream) => uwrite!(
                self.src,
                r#"0, {errors}.New("reading streams not supported yet")"#,
                errors = self.deps.errors(),
            ),

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

            TypeDefKind::Type(t) => self.print_read_ty(t, reader),

            TypeDefKind::Unknown => unreachable!(),
        }
    }

    fn print_write_ty(&mut self, ty: &Type, name: &str, writer: &str) {
        match ty {
            Type::Id(t) => self.print_write_tyid(*t, name, writer),
            Type::Bool => uwrite!(
                self.src,
                r#"func(v bool, w {wrpc}.ByteWriter) error {{
                if !v {{
                    {slog}.Debug("writing `false` byte")
                    return w.WriteByte(0)
                }}
                {slog}.Debug("writing `true` byte")
                return w.WriteByte(1)
            }}({name}, {writer})"#,
                slog = self.deps.slog(),
                wrpc = self.deps.wrpc(),
            ),
            Type::U8 => uwrite!(
                self.src,
                r#"func(v uint8, w {wrpc}.ByteWriter) error {{
                {slog}.Debug("writing u8 byte")
                return w.WriteByte(v)
            }}({name}, {writer})"#,
                slog = self.deps.slog(),
                wrpc = self.deps.wrpc(),
            ),
            Type::U16 => uwrite!(
                self.src,
                r#"func(v uint16, w {wrpc}.ByteWriter) error {{
	            b := make([]byte, {binary}.MaxVarintLen16)
	            i := {binary}.PutUvarint(b, uint64(v))
                {slog}.Debug("writing u16")
	            _, err := w.Write(b[:i])
	            return err
            }}({name}, {writer})"#,
                binary = self.deps.binary(),
                slog = self.deps.slog(),
                wrpc = self.deps.wrpc(),
            ),
            Type::U32 => uwrite!(
                self.src,
                r#"func(v uint32, w {wrpc}.ByteWriter) error {{
	            b := make([]byte, {binary}.MaxVarintLen32)
	            i := {binary}.PutUvarint(b, uint64(v))
                {slog}.Debug("writing u32")
	            _, err := w.Write(b[:i])
	            return err
            }}({name}, {writer})"#,
                binary = self.deps.binary(),
                slog = self.deps.slog(),
                wrpc = self.deps.wrpc(),
            ),
            Type::U64 => uwrite!(
                self.src,
                r#"func(v uint64, w {wrpc}.ByteWriter) error {{
	            b := make([]byte, {binary}.MaxVarintLen64)
	            i := {binary}.PutUvarint(b, uint64(v))
                {slog}.Debug("writing u64")
	            _, err := w.Write(b[:i])
	            return err
            }}({name}, {writer})"#,
                binary = self.deps.binary(),
                slog = self.deps.slog(),
                wrpc = self.deps.wrpc(),
            ),
            Type::S8 => uwrite!(
                self.src,
                r#"{errors}.New("writing s8 not supported yet")"#,
                errors = self.deps.errors(),
            ),
            Type::S16 => uwrite!(
                self.src,
                r#"{errors}.New("writing s16 not supported yet")"#,
                errors = self.deps.errors(),
            ),
            Type::S32 => uwrite!(
                self.src,
                r#"{errors}.New("writing s32 not supported yet")"#,
                errors = self.deps.errors(),
            ),
            Type::S64 => uwrite!(
                self.src,
                r#"{errors}.New("writing s64 not supported yet")"#,
                errors = self.deps.errors(),
            ),
            Type::F32 => uwrite!(
                self.src,
                r#"func(v float32, w {wrpc}.ByteWriter) error {{
                b := make([]byte, 4)
                {binary}.LittleEndian.PutUint32(b, {math}.Float32bits(v))
                {slog}.Debug("writing f32")
	            _, err := w.Write(b)
	            return err
            }}({name}, {writer})"#,
                binary = self.deps.binary(),
                math = self.deps.math(),
                slog = self.deps.slog(),
                wrpc = self.deps.wrpc(),
            ),
            Type::F64 => uwrite!(
                self.src,
                r#"func(v float64, w {wrpc}.ByteWriter) error {{
                b := make([]byte, 4)
                {binary}.LittleEndian.PutUint64(b, {math}.Float64bits(v))
                {slog}.Debug("writing f64")
	            _, err := w.Write(b)
	            return err
            }}({name}, {writer})"#,
                binary = self.deps.binary(),
                math = self.deps.math(),
                slog = self.deps.slog(),
                wrpc = self.deps.wrpc(),
            ),
            Type::Char => uwrite!(
                self.src,
                r#"func(v rune, w {wrpc}.ByteWriter) error {{
	            b := make([]byte, {binary}.MaxVarintLen32)
	            i := {binary}.PutUvarint(b, uint64(v))
                {slog}.Debug("writing char")
	            _, err := w.Write(b[:i])
	            return err
            }}({name}, {writer})"#,
                binary = self.deps.binary(),
                slog = self.deps.slog(),
                wrpc = self.deps.wrpc(),
            ),
            Type::String => {
                let fmt = self.deps.fmt();
                let math = self.deps.math();
                let slog = self.deps.slog();
                let wrpc = self.deps.wrpc();

                uwrite!(
                    self.src,
                    r#"func(v string, w {wrpc}.ByteWriter) error {{
	            n := len(v)
	            if n > {math}.MaxUint32 {{
	            	return {fmt}.Errorf("string byte length of %d overflows a 32-bit integer", n)
	            }}
	            {slog}.Debug("writing string byte length", "len", n)
	            if err := "#
                );
                self.print_write_ty(&Type::U32, "uint32(n)", "w");
                uwrite!(
                    self.src,
                    r#"; err != nil {{
                	return {fmt}.Errorf("failed to write string length of %d: %w", n, err)
                }}
                {slog}.Debug("writing string bytes")
                _, err := w.Write([]byte(v))
                if err != nil {{
                	return {fmt}.Errorf("failed to write string bytes: %w", err)
                }}
                return nil
                }}({name}, {writer})"#
                );
            }
        }
    }

    fn print_write_tyid(&mut self, id: TypeId, name: &str, writer: &str) {
        let ty = &self.resolve.types[id];
        if ty.name.is_some() {
            // TODO: Support async
            uwrite!(self.src, "({name}).WriteTo({writer})");
            return;
        }

        match &ty.kind {
            TypeDefKind::List(ty) => {
                let fmt = self.deps.fmt();
                let math = self.deps.math();
                let slog = self.deps.slog();
                let wrpc = self.deps.wrpc();

                self.push_str("func(v ");
                self.print_list(ty);
                uwrite!(
                    self.src,
                    r#", w {wrpc}.ByteWriter) error {{
	    n := len(v)
	    if n > {math}.MaxUint32 {{
	        return {fmt}.Errorf("list length of %d overflows a 32-bit integer", n)
	    }}
	    {slog}.Debug("writing list length", "len", n)
	    if err := "#
                );
                self.print_write_ty(&Type::U32, "uint32(n)", "w");
                uwrite!(
                    self.src,
                    r#"; err != nil {{
            return {fmt}.Errorf("failed to write list length of %d: %w", n, err)
        }}
        {slog}.Debug("writing list elements")
        for i, e := range v {{
            if err := "#
                );
                self.print_write_ty(ty, "e", "w");
                uwrite!(
                    self.src,
                    r#"; err != nil {{
                return {fmt}.Errorf("failed to write list element %d: %w", i, err)
            }}
        }}
        return nil
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
                    r#", w {wrpc}.ByteWriter) error {{
	    if v == nil {{
	    	{slog}.Debug("writing `option::none` status byte")
	    	if err := w.WriteByte(0); err != nil {{
	    		return {fmt}.Errorf("failed to write `option::none` byte: %w", err)
	    	}}
	    	return nil
	    }}
	    {slog}.Debug("writing `option::some` status byte")
	    if err := w.WriteByte(1); err != nil {{
	    	return {fmt}.Errorf("failed to write `option::some` status byte: %w", err)
	    }}
	    {slog}.Debug("writing `option::some` payload")
        if err := "#
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
                    r#"; err != nil {{
		    return {fmt}.Errorf("failed to write `option::some` payload: %w", err)
	    }}
	    return nil
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
                    r#", w {wrpc}.ByteWriter) error {{
        switch {{
        	case v.Ok == nil && v.Err == nil:
        		return {errors}.New("both result variants cannot be nil")
        	case v.Ok != nil && v.Err != nil:
        		return {errors}.New("exactly one result variant must non-nil")"#
                );
                uwriteln!(
                    self.src,
                    r#"
        	case v.Ok != nil:
        		{slog}.Debug("writing `result::ok` status byte")
        		if err := w.WriteByte(0); err != nil {{
        			return {fmt}.Errorf("failed to write `result::ok` status byte: %w", err)
        		}}"#
                );
                if let Some(ref ty) = ty.ok {
                    uwrite!(
                        self.src,
                        r#"{slog}.Debug("writing `result::ok` payload")
                    if err := "#
                    );
                    self.print_write_ty(ty, "*v.Ok", "w");
                    uwriteln!(
                        self.src,
                        r#"; err != nil {{
        			return {fmt}.Errorf("failed to write `result::ok` payload: %w", err)
        		}}"#
                    );
                }
                uwriteln!(
                    self.src,
                    r#"return nil
        	default:
        		{slog}.Debug("writing `result::err` status byte")
        		if err := w.WriteByte(1); err != nil {{
        			return {fmt}.Errorf("failed to write `result::err` status byte: %w", err)
        		}}"#
                );
                if let Some(ref ty) = ty.err {
                    uwrite!(
                        self.src,
                        r#"{slog}.Debug("writing `result::err` payload")
        		if err := "#
                    );
                    self.print_write_ty(ty, "*v.Err", "w");
                    uwriteln!(
                        self.src,
                        r#"; err != nil {{
        			return {fmt}.Errorf("failed to write `result::err` payload: %w", err)
        		}}"#
                    );
                }

                uwrite!(
                    self.src,
                    r#"return nil
	    }}
    }}({name}, {writer})"#
                );
            }

            TypeDefKind::Variant(_) => panic!("unsupported anonymous variant"),

            TypeDefKind::Tuple(ty) => match ty.types.as_slice() {
                [] => self.push_str("nil"),
                [ty] => self.print_write_ty(ty, name, writer),
                _ => {
                    let wrpc = self.deps.wrpc();

                    self.push_str("func(v ");
                    self.print_tuple(ty, true);
                    uwriteln!(self.src, r#", w {wrpc}.ByteWriter) error {{"#);
                    for (i, ty) in ty.types.iter().enumerate() {
                        let fmt = self.deps.fmt();
                        let slog = self.deps.slog();
                        uwrite!(
                            self.src,
                            r#"{slog}.Debug("writing tuple element {i}")
        if err := "#
                        );
                        self.print_write_ty(ty, &format!("v.V{i}"), "w");
                        uwriteln!(
                            self.src,
                            r#"; err != nil {{
		    return {fmt}.Errorf("failed to write tuple element {i}: %w", err)
	    }}"#
                        );
                    }
                    uwrite!(
                        self.src,
                        r#"return nil
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
            TypeDefKind::Stream(_stream) => uwrite!(
                self.src,
                r#"0, {errors}.New("writing streams not supported yet")"#,
                errors = self.deps.errors(),
            ),

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
                self.print_return_decl(&func.results);
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
            let context = self.deps.context();
            let fmt = self.deps.fmt();
            let slog = self.deps.slog();
            let wrpc = self.deps.wrpc();
            uwriteln!(
                self.src,
                r#"stop{i}, err := c.Serve("{instance}", "{name}", func(ctx {context}.Context, buffer []byte, tx {wrpc}.Transmitter, inv {wrpc}.IncomingInvocation) error {{
        {slog}.DebugContext(ctx, "subscribing for `{instance}.{name}` parameters")

		payload := make(chan []byte)
		stop, err := inv.Subscribe(func(ctx {context}.Context, buf []byte) {{
			payload <- buf
		}})
		if err != nil {{
			return err
		}}
		defer func() {{
			if err := stop(); err != nil {{
                {slog}.ErrorContext(ctx, "failed to stop parameter subscription", "err", err)
			}}
		}}()

        // TODO: Handle async parameters

        {slog}.DebugContext(ctx, "accepting handshake")
        if err := inv.Accept(ctx, nil); err != nil {{
            return {fmt}.Errorf("failed to complete handshake: %w", err)
        }}"#,
            );
            if !params.is_empty() {
                uwriteln!(self.src, "r := {wrpc}.NewChanReader(ctx, payload, buffer)",);
            }
            for (i, (_, ty)) in params.iter().enumerate() {
                uwrite!(
                    self.src,
                    r#"{slog}.DebugContext(ctx, "reading parameter", "i", {i})
        p{i}, err := "#
                );
                self.print_read_ty(ty, "r");
                self.push_str("\n");
                uwriteln!(
                    self.src,
                    r#"if err != nil {{ return {fmt}.Errorf("failed to read parameter {i}") }}"#,
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

            uwriteln!(self.src, r#"var buf {bytes}.Buffer"#,);
            for (i, ty) in results.iter_types().enumerate() {
                self.push_str("if err :=");
                self.print_write_ty(ty, &format!("r{i}"), "&buf");
                self.push_str("; err != nil {\n");
                uwriteln!(
                    self.src,
                    r#"return {fmt}.Errorf("failed to write result value {i}: %w", err)"#,
                );
                self.src.push_str("}\n");
            }

            uwriteln!(
                self.src,
                r#"{slog}.DebugContext(ctx, "transmitting `{instance}.{name}` result")"#,
            );
            uwriteln!(
                self.src,
                r#"if err := tx.Transmit({context}.Background(), buf.Bytes()); err != nil {{
                    return {fmt}.Errorf("failed to transmit result: %w", err)
                }}"#,
            );
            self.push_str("return nil\n");

            uwriteln!(
                self.src,
                r#"}})
                if err != nil {{
                    return nil, {fmt}.Errorf("failed to serve `{instance}.{name}`: %w", err)
                }}
                stops = append(stops, stop{i})"#,
                fmt = self.deps.fmt(),
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

        let mut sig = FnSig::default();
        match func.kind {
            FunctionKind::Freestanding => {}
            FunctionKind::Method(id) | FunctionKind::Static(id) | FunctionKind::Constructor(id) => {
                let name = self.resolve.types[id].name.as_ref().unwrap();
                let name = to_upper_camel_case(name);
                uwriteln!(self.src, "impl {name} {{");
                sig.use_item_name = true;
                if let FunctionKind::Method(_) = &func.kind {
                    sig.self_arg = Some("&self".into());
                    sig.self_is_first_param = true;
                }
            }
        }
        let _params = self.print_signature(func, &sig);
        self.src.push_str("{\n");
        self.src.push_str("wrpc__.NewInvocation(");
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
        self.src.push_str("\")\n");
        uwriteln!(
            self.src,
            r#"
        //if err != nil {{
        //    err__ = fmt.Sprintf("failed to invoke `{}`: %w", txErr__)
        //    return 
        //}}
        //wrpc__.1.await.context("failed to transmit parameters")?;
        //Ok(tx__)
        panic("not supported yet")
        return
    }}"#,
            func.name
        );

        match func.kind {
            FunctionKind::Freestanding => {}
            FunctionKind::Method(_) | FunctionKind::Static(_) | FunctionKind::Constructor(_) => {
                self.src.push_str("}\n");
            }
        }
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

    fn print_signature(&mut self, func: &Function, sig: &FnSig) -> Vec<String> {
        let params = self.print_docs_and_params(func, sig);
        if let FunctionKind::Constructor(_) = &func.kind {
            uwrite!(self.src, " (Self, error)",);
        } else {
            self.print_return_decl(&func.results);
        }
        params
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
        uwrite!(self.src, "(ctx__ {}.Context, ", self.deps.context());
        if self.in_import {
            uwrite!(self.src, "wrpc__ {}.Client, ", self.deps.wrpc());
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

    fn print_return_decl(&mut self, results: &Results) {
        self.src.push_str(" (");
        for (i, ty) in results.iter_types().enumerate() {
            uwrite!(self.src, "r{i}__ ");
            self.print_opt_ty(ty, true);
            self.src.push_str(", ");
        }
        self.push_str("err__ error) ");
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
            TypeDefKind::Future(ty) => {
                let wrpc = self.deps.wrpc();
                self.push_str(wrpc);
                self.push_str(".Receiver[");
                self.print_optional_ty(ty.as_ref());
                self.push_str("]");
            }
            TypeDefKind::Stream(stream) => {
                let wrpc = self.deps.wrpc();
                self.push_str(wrpc);
                self.push_str(".Receiver[[]");
                self.print_optional_ty(stream.element.as_ref());
                self.push_str("]");
            }

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
        self.print_ty(ty, true);
    }

    fn int_repr(&mut self, repr: Int) {
        self.push_str(int_repr(repr));
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

func (v *{name}) WriteTo(w {wrpc}.ByteWriter) error {{"#
            );
            for Field { name, ty, .. } in fields {
                let fmt = self.deps.fmt();
                let slog = self.deps.slog();
                uwrite!(
                    self.src,
                    r#"{slog}.Debug("writing field", "name", "{name}")
        if err := "#
                );
                let ident = name.to_upper_camel_case();
                self.print_write_ty(ty, &format!("v.{ident}"), "w");
                uwriteln!(
                    self.src,
                    r#"; err != nil {{
		    return {fmt}.Errorf("failed to write `{name}` field: %w", err)
	    }}"#
                );
            }
            self.push_str("return nil\n");
            self.push_str("}\n");

            uwriteln!(
                self.src,
                r#"func Read{name}(r {wrpc}.ByteReader) (*{name}, error) {{
    v := &{name}{{}}
    var err error"#
            );
            for Field { name, ty, .. } in fields {
                let fmt = self.deps.fmt();
                let slog = self.deps.slog();
                let ident = name.to_upper_camel_case();
                uwrite!(
                    self.src,
                    r#"{slog}.Debug("reading field", "name", "{name}")
    v.{ident}, err = "#
                );
                self.print_read_ty(ty, "r");
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

    fn type_resource(&mut self, _id: TypeId, _name: &str, _docs: &Docs) {
        // TODO: Support resources
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

    fn type_flags(&mut self, _id: TypeId, name: &str, flags: &Flags, docs: &Docs) {
        self.src.push_str("bitflags::bitflags! {\n");
        self.godoc(docs);
        let repr = RustFlagsRepr::new(flags);
        let name = to_upper_camel_case(name);
        self.src.push_str(&format!(
            "#[derive(PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Clone, Copy)]\npub struct {name}: {repr} {{\n",
        ));
        for (i, flag) in flags.flags.iter().enumerate() {
            self.godoc(&flag.docs);
            self.src.push_str(&format!(
                "const {} = 1 << {};\n",
                flag.name.to_shouty_snake_case(),
                i,
            ));
        }
        self.src.push_str("}\n");
        self.src.push_str("}\n");
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
                r#"func (v *{name}) WriteTo(w {wrpc}.ByteWriter) error {{"#,
            );
            self.push_str("if err := ");
            match variant.tag() {
                Int::U8 => {
                    self.print_write_ty(&Type::U8, "uint8(v.discriminant)", "w");
                }
                Int::U16 => {
                    self.print_write_ty(&Type::U16, "uint16(v.discriminant)", "w");
                }
                Int::U32 => {
                    self.print_write_ty(&Type::U32, "uint32(v.discriminant)", "w");
                }
                Int::U64 => {
                    self.print_write_ty(&Type::U64, "uint64(v.discriminant)", "w");
                }
            }
            self.push_str("; err != nil { return ");
            self.push_str(fmt);
            self.push_str(".Errorf(\"failed to write discriminant: %w\", err)\n}\n");
            self.push_str("switch v.discriminant {\n");
            for Case {
                name: case_name,
                ty,
                ..
            } in &variant.cases
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
                    self.push_str("if !ok { return ");
                    let errors = self.deps.errors();
                    self.push_str(errors);
                    self.push_str(".New(\"invalid payload\") }\n");
                    self.push_str("if err := ");
                    self.print_write_ty(ty, "payload", "w");
                    self.push_str("; err != nil { return ");
                    self.push_str(fmt);
                    self.push_str(".Errorf(\"failed to write payload: %w\", err)\n}\n");
                }
            }
            self.push_str("default: panic(\"invalid variant\")\n}\n");
            self.push_str("return nil\n");
            self.push_str("}\n");

            uwrite!(
                self.src,
                r#"func Read{name}(r {wrpc}.ByteReader) (*{name}, error) {{
    disc, err := "#,
            );
            match variant.tag() {
                Int::U8 => {
                    self.print_read_ty(&Type::U8, "r");
                }
                Int::U16 => {
                    self.print_read_ty(&Type::U16, "r");
                }
                Int::U32 => {
                    self.print_read_ty(&Type::U32, "r");
                }
                Int::U64 => {
                    self.print_read_ty(&Type::U64, "r");
                }
            }
            self.push_str("\n");
            self.push_str("if err != nil {\n");
            self.push_str("return nil, ");
            self.push_str(fmt);
            self.push_str(".Errorf(\"failed to read discriminant: %w\", err)\n}\n");
            uwriteln!(self.src, "switch {name}Discriminant(disc) {{");
            for Case {
                name: case_name,
                ty,
                ..
            } in &variant.cases
            {
                self.push_str("case ");
                self.push_str(&name);
                self.push_str("Discriminant_");
                self.push_str(&case_name.to_upper_camel_case());
                self.push_str(":\n");
                if let Some(ty) = ty {
                    self.push_str("payload, err := ");
                    self.print_read_ty(ty, "r");
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
                r#"func (v {name}) WriteTo(w {wrpc}.ByteWriter) error {{"#,
            );
            self.push_str("if err := ");
            match enum_.tag() {
                Int::U8 => {
                    self.print_write_ty(&Type::U8, "uint8(v)", "w");
                }
                Int::U16 => {
                    self.print_write_ty(&Type::U16, "uint16(v)", "w");
                }
                Int::U32 => {
                    self.print_write_ty(&Type::U32, "uint32(v)", "w");
                }
                Int::U64 => {
                    self.print_write_ty(&Type::U64, "uint64(v)", "w");
                }
            }
            self.push_str("; err != nil { return ");
            self.push_str(fmt);
            self.push_str(".Errorf(\"failed to write discriminant: %w\", err)\n}\n");
            self.push_str("return nil\n");
            self.push_str("}\n");

            uwrite!(
                self.src,
                r#"func Read{name}(r {wrpc}.ByteReader) (v {name}, err error) {{
    disc, err := "#,
            );
            match enum_.tag() {
                Int::U8 => {
                    self.print_read_ty(&Type::U8, "r");
                }
                Int::U16 => {
                    self.print_read_ty(&Type::U16, "r");
                }
                Int::U32 => {
                    self.print_read_ty(&Type::U32, "r");
                }
                Int::U64 => {
                    self.print_read_ty(&Type::U64, "r");
                }
            }
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
