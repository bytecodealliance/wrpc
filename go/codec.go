package wrpc

import (
	"encoding/binary"
	"errors"
	"fmt"
	"io"
	"math"

	"github.com/tetratelabs/wabin/leb128"
)

func EncodeInt16(v int16) []byte {
	return leb128.EncodeInt32(int32(v))
}

func EncodeInt32(v int32) []byte {
	return leb128.EncodeInt32(v)
}

func EncodeInt64(v int64) []byte {
	return leb128.EncodeInt64(v)
}

func EncodeInt(v int) []byte {
	return leb128.EncodeInt64(int64(v))
}

func PutUint16(buf []byte, x uint16) {
	binary.PutUvarint(buf, uint64(x))
}

func AppendUint16(buf []byte, x uint16) []byte {
	return binary.AppendUvarint(buf, uint64(x))
}

func PutUint32(buf []byte, x uint32) {
	binary.PutUvarint(buf, uint64(x))
}

func AppendUint32(buf []byte, x uint32) []byte {
	return binary.AppendUvarint(buf, uint64(x))
}

func PutUint64(buf []byte, x uint64) {
	binary.PutUvarint(buf, x)
}

func AppendUint64(buf []byte, x uint64) []byte {
	return binary.AppendUvarint(buf, x)
}

func AppendFloat32(buf []byte, x float32) []byte {
	return binary.LittleEndian.AppendUint32(buf, math.Float32bits(x))
}

func PutFloat32(buf []byte, x float32) {
	binary.LittleEndian.PutUint32(buf, math.Float32bits(x))
}

func AppendFloat64(buf []byte, x float64) []byte {
	return binary.LittleEndian.AppendUint64(buf, math.Float64bits(x))
}

func PutFloat64(buf []byte, x float64) {
	binary.LittleEndian.PutUint64(buf, math.Float64bits(x))
}

func AppendString(buf []byte, s string) ([]byte, error) {
	n := len(s)
	if n > math.MaxUint32 {
		return nil, fmt.Errorf("response UTF-8 string byte length of %d overflows u32", n)
	}
	return append(append(buf, leb128.EncodeUint32(uint32(n))...), s...), nil
}

func WriteString(w io.Writer, s string) (int, error) {
	n := len(s)
	if n > math.MaxUint32 {
		return 0, fmt.Errorf("response UTF-8 string byte length of %d overflows u32", n)
	}
	n, err := w.Write(leb128.EncodeUint32(uint32(n)))
	if err != nil {
		return n, fmt.Errorf("failed to write string length of %d: %w", n, err)
	}
	sn, err := w.Write([]byte(s))
	if sn > math.MaxInt-n {
		return math.MaxInt, errors.New("encoded string length overflows int")
	}
	n += sn
	if err != nil {
		return n, fmt.Errorf("failed to write string: %w", err)
	}
	return n, nil
}

// ReadUint64 reads an encoded uint64 from r and returns it.
// The error is [io.EOF] only if no bytes were read.
// If an [io.EOF] happens after reading some but not all the bytes,
// ReadUvarint returns [io.ErrUnexpectedEOF].
func ReadUint64(r io.ByteReader) (uint64, error) {
	return binary.ReadUvarint(r)
}

type ByteReader interface {
	io.ByteReader
	io.Reader
}

func ReadString(r ByteReader) (string, error) {
	n, err := ReadUint32(r)
	if err != nil {
		return "", fmt.Errorf("failed to read string length: %w", err)
	}
	b := make([]byte, int(n))
	_, err = r.Read(b)
	if err != nil {
		return "", fmt.Errorf("failed to read string: %w", err)
	}
	return string(b), nil
}

// NOTE: Below is adapted from https://cs.opensource.google/go/go/+/refs/tags/go1.22.2:src/encoding/binary/varint.go;l=128-153

// maxVarintLenN is the maximum length of a varint-encoded N-bit integer.
const (
	maxVarintLen16 = 3
	maxVarintLen32 = 5
)

var errOverflow16 = errors.New("wrpc: varint overflows a 16-bit integer")

// ReadUint16 reads an encoded uint16 from r and returns it.
// The error is [io.EOF] only if no bytes were read.
// If an [io.EOF] happens after reading some but not all the bytes,
// ReadUvarint returns [io.ErrUnexpectedEOF].
func ReadUint16(r io.ByteReader) (uint16, error) {
	var x uint16
	var s uint
	for i := 0; i < maxVarintLen16; i++ {
		b, err := r.ReadByte()
		if err != nil {
			if i > 0 && err == io.EOF {
				err = io.ErrUnexpectedEOF
			}
			return x, err
		}
		if b < 0x80 {
			if i == maxVarintLen16-1 && b > 1 {
				return x, errOverflow16
			}
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
// ReadUvarint returns [io.ErrUnexpectedEOF].
func ReadUint32(r io.ByteReader) (uint32, error) {
	var x uint32
	var s uint
	for i := 0; i < maxVarintLen32; i++ {
		b, err := r.ReadByte()
		if err != nil {
			if i > 0 && err == io.EOF {
				err = io.ErrUnexpectedEOF
			}
			return x, err
		}
		if b < 0x80 {
			if i == maxVarintLen32-1 && b > 1 {
				return x, errOverflow32
			}
			return x | uint32(b)<<s, nil
		}
		x |= uint32(b&0x7f) << s
		s += 7
	}
	return x, errOverflow32
}
