package wrpc

import (
	"encoding/binary"
	"errors"
	"fmt"
	"io"
	"math"
	"slices"
	"unicode/utf8"
)

const minReadSize = 512

// AppendUint16 appends the encoding of `x` to `buf`
func AppendUint16(buf []byte, x uint16) []byte {
	return binary.AppendUvarint(buf, uint64(x))
}

// AppendUint32 appends the encoding of `x` to `buf`
func AppendUint32(buf []byte, x uint32) []byte {
	return binary.AppendUvarint(buf, uint64(x))
}

// AppendUint64 appends the encoding of `x` to `buf`
func AppendUint64(buf []byte, x uint64) []byte {
	return binary.AppendUvarint(buf, x)
}

func float32Bits(x float32) uint32 {
	if math.IsNaN(float64(x)) {
		return 0x7fc0_0000
	}
	return math.Float32bits(x)
}

func float64Bits(x float64) uint64 {
	if math.IsNaN(x) {
		return 0x7ff8_0000_0000_0000
	}
	return math.Float64bits(x)
}

// AppendFloat32 appends the encoding of `x` to `buf`, NaN values are canonicalized
func AppendFloat32(buf []byte, x float32) []byte {
	return binary.LittleEndian.AppendUint32(buf, float32Bits(x))
}

// AppendFloat64 appends the encoding of `x` to `buf`, NaN values are canonicalized
func AppendFloat64(buf []byte, x float64) []byte {
	return binary.LittleEndian.AppendUint64(buf, float64Bits(x))
}

// ReadUint64 reads an encoded uint64 from r and returns it.
// The error is [io.EOF] only if no bytes were read.
// If an [io.EOF] happens after reading some but not all the bytes,
// ReadUint64 returns [io.ErrUnexpectedEOF].
func ReadUint64(r io.ByteReader) (uint64, error) {
	return binary.ReadUvarint(r)
}

var errOverflow16 = errors.New("wrpc: varint overflows a 16-bit integer")

// ReadUint16 reads an encoded uint16 from r and returns it.
// The error is [io.EOF] only if no bytes were read.
// If an [io.EOF] happens after reading some but not all the bytes,
// ReadUint16 returns [io.ErrUnexpectedEOF].
func ReadUint16(r io.ByteReader) (uint16, error) {
	var x uint16
	var s uint8
	for i := range 3 {
		b, err := r.ReadByte()
		if err != nil {
			if i > 0 && err == io.EOF {
				err = io.ErrUnexpectedEOF
			}
			return x, err
		}
		if s == 14 && b > 0x03 {
			return x, errOverflow16
		}
		if b < 0x80 {
			return x | uint16(b)<<s, nil
		}
		x |= uint16(b&0x7f) << s
		s += 7
	}
	return x, errOverflow16
}

var errOverflow32 = errors.New("wrpc: varint overflows a 32-bit integer")

// ReadUint32 reads an encoded uint32 from r and returns it.
// The error is [io.EOF] only if no bytes were read.
// If an [io.EOF] happens after reading some but not all the bytes,
// ReadUint32 returns [io.ErrUnexpectedEOF].
func ReadUint32(r io.ByteReader) (uint32, error) {
	var x uint32
	var s uint8
	for i := range 5 {
		b, err := r.ReadByte()
		if err != nil {
			if i > 0 && err == io.EOF {
				err = io.ErrUnexpectedEOF
			}
			return x, err
		}
		if s == 28 && b > 0x0f {
			return x, errOverflow32
		}
		if b < 0x80 {
			return x | uint32(b)<<s, nil
		}
		x |= uint32(b&0x7f) << s
		s += 7
	}
	return x, errOverflow32
}

// ReadBytes reads exactly `n` bytes from `r` and returns them,
// growing the buffer as the bytes arrive
func ReadBytes(r io.Reader, n uint32) ([]byte, error) {
	b := make([]byte, 0)
	for n > 0 {
		k := int(min(n, uint32(max(len(b), minReadSize))))
		b = slices.Grow(b, k)
		if _, err := io.ReadFull(r, b[len(b):len(b)+k]); err != nil {
			if err == io.EOF {
				err = io.ErrUnexpectedEOF
			}
			return nil, err
		}
		b = b[:len(b)+k]
		n -= uint32(k)
	}
	return b, nil
}

// ReadByteList reads an encoded `list<u8>` from `r` and returns it
func ReadByteList(r ByteReader) ([]byte, error) {
	n, err := ReadUint32(r)
	if err != nil {
		return nil, fmt.Errorf("failed to read list length: %w", err)
	}
	b, err := ReadBytes(r, n)
	if err != nil {
		return nil, fmt.Errorf("failed to read list bytes: %w", err)
	}
	return b, nil
}

// ReadString reads an encoded `string` from `r` and returns it
func ReadString(r ByteReader) (string, error) {
	n, err := ReadUint32(r)
	if err != nil {
		return "", fmt.Errorf("failed to read string length: %w", err)
	}
	b, err := ReadBytes(r, n)
	if err != nil {
		return "", fmt.Errorf("failed to read string: %w", err)
	}
	if !utf8.Valid(b) {
		return "", errors.New("string is not valid UTF-8")
	}
	return string(b), nil
}
