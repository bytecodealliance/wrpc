package wrpc

import (
	"encoding/binary"
	"errors"
	"fmt"
	"io"
	"math"
	"unicode/utf8"
)

var errOverflowSigned = errors.New("signed integer overflow")

// BoolCodec is the Codec of `bool` values
type BoolCodec struct{}

func (BoolCodec) Encode(v bool, dst []byte) ([]byte, Transmitter, error) {
	if v {
		return append(dst, 1), nil, nil
	}
	return append(dst, 0), nil, nil
}

func (BoolCodec) Decode(r ByteReader) (bool, Receiver, error) {
	b, err := r.ReadByte()
	if err != nil {
		return false, nil, err
	}
	switch b {
	case 0:
		return false, nil, nil
	case 1:
		return true, nil, nil
	default:
		return false, nil, fmt.Errorf("invalid bool value `%d`", b)
	}
}

// U8Codec is the Codec of `u8` values
type U8Codec struct{}

func (U8Codec) Encode(v uint8, dst []byte) ([]byte, Transmitter, error) {
	return append(dst, v), nil, nil
}

func (U8Codec) Decode(r ByteReader) (uint8, Receiver, error) {
	v, err := r.ReadByte()
	return v, nil, err
}

// U16Codec is the Codec of `u16` values
type U16Codec struct{}

func (U16Codec) Encode(v uint16, dst []byte) ([]byte, Transmitter, error) {
	return AppendUint16(dst, v), nil, nil
}

func (U16Codec) Decode(r ByteReader) (uint16, Receiver, error) {
	v, err := ReadUint16(r)
	return v, nil, err
}

// U32Codec is the Codec of `u32` values
type U32Codec struct{}

func (U32Codec) Encode(v uint32, dst []byte) ([]byte, Transmitter, error) {
	return AppendUint32(dst, v), nil, nil
}

func (U32Codec) Decode(r ByteReader) (uint32, Receiver, error) {
	v, err := ReadUint32(r)
	return v, nil, err
}

// U64Codec is the Codec of `u64` values
type U64Codec struct{}

func (U64Codec) Encode(v uint64, dst []byte) ([]byte, Transmitter, error) {
	return AppendUint64(dst, v), nil, nil
}

func (U64Codec) Decode(r ByteReader) (uint64, Receiver, error) {
	v, err := ReadUint64(r)
	return v, nil, err
}

// S8Codec is the Codec of `s8` values
type S8Codec struct{}

func (S8Codec) Encode(v int8, dst []byte) ([]byte, Transmitter, error) {
	return append(dst, byte(v)), nil, nil
}

func (S8Codec) Decode(r ByteReader) (int8, Receiver, error) {
	v, err := r.ReadByte()
	return int8(v), nil, err
}

// S16Codec is the Codec of `s16` values
type S16Codec struct{}

func (S16Codec) Encode(v int16, dst []byte) ([]byte, Transmitter, error) {
	return appendSigned(dst, int64(v)), nil, nil
}

func (S16Codec) Decode(r ByteReader) (int16, Receiver, error) {
	v, err := readSigned(r, 16)
	return int16(v), nil, err
}

// S32Codec is the Codec of `s32` values
type S32Codec struct{}

func (S32Codec) Encode(v int32, dst []byte) ([]byte, Transmitter, error) {
	return appendSigned(dst, int64(v)), nil, nil
}

func (S32Codec) Decode(r ByteReader) (int32, Receiver, error) {
	v, err := readSigned(r, 32)
	return int32(v), nil, err
}

// S64Codec is the Codec of `s64` values
type S64Codec struct{}

func (S64Codec) Encode(v int64, dst []byte) ([]byte, Transmitter, error) {
	return appendSigned(dst, v), nil, nil
}

func (S64Codec) Decode(r ByteReader) (int64, Receiver, error) {
	v, err := readSigned(r, 64)
	return v, nil, err
}

func appendSigned(dst []byte, v int64) []byte {
	for {
		b := byte(v & 0x7f)
		v >>= 7
		if v == 0 && b&0x40 == 0 || v == -1 && b&0x40 != 0 {
			return append(dst, b)
		}
		dst = append(dst, b|0x80)
	}
}

// readSigned reads a signed LEB128 integer of `bits` bits
func readSigned(r ByteReader, bits uint) (int64, error) {
	var v int64
	for s := uint(0); ; s += 7 {
		b, err := r.ReadByte()
		if err != nil {
			if s > 0 && err == io.EOF {
				err = io.ErrUnexpectedEOF
			}
			return 0, err
		}
		x := int64(int8(b<<1) >> 1)
		if s+7 >= bits {
			n := bits - s
			if b&0x80 != 0 || x < -(1<<(n-1)) || x >= 1<<(n-1) {
				return 0, errOverflowSigned
			}
			return v | x<<s, nil
		}
		if b&0x80 == 0 {
			return v | x<<s, nil
		}
		v |= int64(b&0x7f) << s
	}
}

// F32Codec is the Codec of `f32` values
type F32Codec struct{}

func (F32Codec) Encode(v float32, dst []byte) ([]byte, Transmitter, error) {
	return AppendFloat32(dst, v), nil, nil
}

func (F32Codec) Decode(r ByteReader) (float32, Receiver, error) {
	var b [4]byte
	if _, err := io.ReadFull(r, b[:]); err != nil {
		return 0, nil, err
	}
	return math.Float32frombits(binary.LittleEndian.Uint32(b[:])), nil, nil
}

// F64Codec is the Codec of `f64` values
type F64Codec struct{}

func (F64Codec) Encode(v float64, dst []byte) ([]byte, Transmitter, error) {
	return AppendFloat64(dst, v), nil, nil
}

func (F64Codec) Decode(r ByteReader) (float64, Receiver, error) {
	var b [8]byte
	if _, err := io.ReadFull(r, b[:]); err != nil {
		return 0, nil, err
	}
	return math.Float64frombits(binary.LittleEndian.Uint64(b[:])), nil, nil
}

// CharCodec is the Codec of `char` values
type CharCodec struct{}

func (CharCodec) Encode(v rune, dst []byte) ([]byte, Transmitter, error) {
	if !utf8.ValidRune(v) {
		return nil, nil, fmt.Errorf("invalid char `%d`", v)
	}
	return utf8.AppendRune(dst, v), nil, nil
}

func (CharCodec) Decode(r ByteReader) (rune, Receiver, error) {
	var b [utf8.UTFMax]byte
	var err error
	if b[0], err = r.ReadByte(); err != nil {
		return 0, nil, err
	}
	var n int
	switch {
	case b[0] < 0x80:
		return rune(b[0]), nil, nil
	case b[0]&0xe0 == 0xc0:
		n = 2
	case b[0]&0xf0 == 0xe0:
		n = 3
	case b[0]&0xf8 == 0xf0:
		n = 4
	default:
		return 0, nil, errors.New("char is not valid UTF-8")
	}
	if _, err := io.ReadFull(r, b[1:n]); err != nil {
		return 0, nil, unexpectedEOF(err)
	}
	v, size := utf8.DecodeRune(b[:n])
	if size != n {
		return 0, nil, errors.New("char is not valid UTF-8")
	}
	return v, nil, nil
}

// BytesCodec is the Codec of `list<u8>` values
type BytesCodec struct{}

func (BytesCodec) Encode(v []byte, dst []byte) ([]byte, Transmitter, error) {
	if uint64(len(v)) > math.MaxUint32 {
		return nil, nil, errors.New("list length overflows uint32")
	}
	return append(AppendUint32(dst, uint32(len(v))), v...), nil, nil
}

func (BytesCodec) Decode(r ByteReader) ([]byte, Receiver, error) {
	v, err := ReadByteList(r)
	return v, nil, err
}

// OwnCodec is the Codec of `own<T>` resource handles
type OwnCodec[T any] struct{}

func (OwnCodec[T]) Encode(v Own[T], dst []byte) ([]byte, Transmitter, error) {
	return BytesCodec{}.Encode(v, dst)
}

func (OwnCodec[T]) Decode(r ByteReader) (Own[T], Receiver, error) {
	v, err := ReadByteList(r)
	return v, nil, err
}

// BorrowCodec is the Codec of `borrow<T>` resource handles
type BorrowCodec[T any] struct{}

func (BorrowCodec[T]) Encode(v Borrow[T], dst []byte) ([]byte, Transmitter, error) {
	return BytesCodec{}.Encode(v, dst)
}

func (BorrowCodec[T]) Decode(r ByteReader) (Borrow[T], Receiver, error) {
	v, err := ReadByteList(r)
	return v, nil, err
}

// FlagsCodec is the Codec of `flags` values with N flags, up to 64, as a bit set where bit `i` is flag `i`
type FlagsCodec struct {
	N uint8
}

func (c FlagsCodec) Encode(v uint64, dst []byte) ([]byte, Transmitter, error) {
	if v>>c.N != 0 {
		return nil, nil, errors.New("bit not associated with any flag is set")
	}
	for i := uint8(0); i < c.N; i += 8 {
		dst = append(dst, byte(v>>i))
	}
	return dst, nil, nil
}

func (c FlagsCodec) Decode(r ByteReader) (uint64, Receiver, error) {
	var v uint64
	for i := uint8(0); i < c.N; i += 8 {
		b, err := r.ReadByte()
		if err != nil {
			if i > 0 {
				err = unexpectedEOF(err)
			}
			return 0, nil, err
		}
		v |= uint64(b) << i
	}
	if v>>c.N != 0 {
		return 0, nil, errors.New("bit not associated with any flag is set")
	}
	return v, nil, nil
}
