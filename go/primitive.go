package wrpc

import (
	"encoding/binary"
	"errors"
	"fmt"
	"io"
	"log/slog"
	"math"
)

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

func WriteUint8(v uint8, w ByteWriter) error {
	return w.WriteByte(v)
}

func WriteUint16(v uint16, w ByteWriter) error {
	b := make([]byte, binary.MaxVarintLen16)
	i := binary.PutUvarint(b, uint64(v))
	_, err := w.Write(b[:i])
	return err
}

func WriteUint32(v uint32, w ByteWriter) error {
	b := make([]byte, binary.MaxVarintLen32)
	i := binary.PutUvarint(b, uint64(v))
	_, err := w.Write(b[:i])
	return err
}

func WriteUint64(v uint64, w ByteWriter) error {
	b := make([]byte, binary.MaxVarintLen64)
	i := binary.PutUvarint(b, uint64(v))
	_, err := w.Write(b[:i])
	return err
}

func WriteString(v string, w ByteWriter) error {
	n := len(v)
	if n > math.MaxUint32 {
		return fmt.Errorf("string byte length of %d overflows a 32-bit integer", n)
	}
	slog.Debug("writing string byte length", "len", n)
	if err := WriteUint32(uint32(n), w); err != nil {
		return fmt.Errorf("failed to write string length of %d: %w", n, err)
	}
	slog.Debug("writing string bytes")
	_, err := w.Write([]byte(v))
	if err != nil {
		return fmt.Errorf("failed to write string bytes: %w", err)
	}
	return nil
}

// ReadString reads a string from `r` and returns it
func ReadString(r ByteReader) (string, error) {
	slog.Debug("reading string length")
	n, err := ReadUint32(r)
	if err != nil {
		return "", fmt.Errorf("failed to read string length: %w", err)
	}

	b := make([]byte, int(n))
	slog.Debug("reading string bytes", "len", n)
	rn, err := r.Read(b)
	if err != nil {
		return "", fmt.Errorf("failed to read string: %w", err)
	}
	if rn > int(n) {
		return "", fmt.Errorf("invalid amount of string bytes read, expected %d, got %d", n, rn)
	}
	slog.Debug("read string bytes", "buf", b)
	return string(b), nil
}

// ReadUint64 reads an encoded uint64 from r and returns it.
// The error is [io.EOF] only if no bytes were read.
// If an [io.EOF] happens after reading some but not all the bytes,
// ReadUvarint returns [io.ErrUnexpectedEOF].
func ReadUint64(r ByteReader) (uint64, error) {
	return binary.ReadUvarint(r)
}

var errOverflow16 = errors.New("wrpc: varint overflows a 16-bit integer")

// ReadUint16 reads an encoded uint16 from r and returns it.
// The error is [io.EOF] only if no bytes were read.
// If an [io.EOF] happens after reading some but not all the bytes,
// ReadUvarint returns [io.ErrUnexpectedEOF].
func ReadUint16(r ByteReader) (uint16, error) {
	var x uint16
	var s uint8
	for i := 0; i < 3; i++ {
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
// ReadUvarint returns [io.ErrUnexpectedEOF].
func ReadUint32(r ByteReader) (uint32, error) {
	var x uint32
	var s uint8
	for i := 0; i < 5; i++ {
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
