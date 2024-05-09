package wrpc

import (
	"bytes"
	"context"
	"encoding/binary"
	"errors"
	"fmt"
	"io"
	"log/slog"
	"math"
)

func Slice[T any](v []T) *[]T {
	if v == nil {
		return nil
	}
	return &v
}

type Tuple2[T0, T1 any] struct {
	V0 T0
	V1 T1
}

func (v *Tuple2[T0, T1]) WriteTo(w ByteWriter, f0 func(T0, ByteWriter) error, f1 func(T1, ByteWriter) error) error {
	slog.Debug("writing tuple element 0")
	if err := f0(v.V0, w); err != nil {
		return fmt.Errorf("failed to write tuple element 0: %w", err)
	}
	slog.Debug("writing tuple element 1")
	if err := f1(v.V1, w); err != nil {
		return fmt.Errorf("failed to write tuple element 1: %w", err)
	}
	return nil
}

type Tuple3[T0, T1, T2 any] struct {
	V0 T0
	V1 T1
	V2 T2
}

func (v *Tuple3[T0, T1, T2]) WriteTo(w ByteWriter, f0 func(T0, ByteWriter) error, f1 func(T1, ByteWriter) error, f2 func(T2, ByteWriter) error) error {
	slog.Debug("writing tuple element 0")
	if err := f0(v.V0, w); err != nil {
		return fmt.Errorf("failed to write tuple element 0: %w", err)
	}
	slog.Debug("writing tuple element 1")
	if err := f1(v.V1, w); err != nil {
		return fmt.Errorf("failed to write tuple element 1: %w", err)
	}
	slog.Debug("writing tuple element 2")
	if err := f2(v.V2, w); err != nil {
		return fmt.Errorf("failed to write tuple element 2: %w", err)
	}
	return nil
}

type Tuple4[T0, T1, T2, T3 any] struct {
	V0 T0
	V1 T1
	V2 T2
	V3 T3
}

type Tuple5[T0, T1, T2, T3, T4 any] struct {
	V0 T0
	V1 T1
	V2 T2
	V3 T3
	V4 T4
}

type Tuple6[T0, T1, T2, T3, T4, T5 any] struct {
	V0 T0
	V1 T1
	V2 T2
	V3 T3
	V4 T4
	V5 T5
}

type Tuple7[T0, T1, T2, T3, T4, T5, T6 any] struct {
	V0 T0
	V1 T1
	V2 T2
	V3 T3
	V4 T4
	V5 T5
	V6 T6
}

type Tuple8[T0, T1, T2, T3, T4, T5, T6, T7 any] struct {
	V0 T0
	V1 T1
	V2 T2
	V3 T3
	V4 T4
	V5 T5
	V6 T6
	V7 T7
}

type Tuple9[T0, T1, T2, T3, T4, T5, T6, T7, T8 any] struct {
	V0 T0
	V1 T1
	V2 T2
	V3 T3
	V4 T4
	V5 T5
	V6 T6
	V7 T7
	V8 T8
}

type Tuple10[T0, T1, T2, T3, T4, T5, T6, T7, T8, T9 any] struct {
	V0 T0
	V1 T1
	V2 T2
	V3 T3
	V4 T4
	V5 T5
	V6 T6
	V7 T7
	V8 T8
	V9 T9
}

type Tuple11[T0, T1, T2, T3, T4, T5, T6, T7, T8, T9, T10 any] struct {
	V0  T0
	V1  T1
	V2  T2
	V3  T3
	V4  T4
	V5  T5
	V6  T6
	V7  T7
	V8  T8
	V9  T9
	V10 T10
}

type Tuple12[T0, T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11 any] struct {
	V0  T0
	V1  T1
	V2  T2
	V3  T3
	V4  T4
	V5  T5
	V6  T6
	V7  T7
	V8  T8
	V9  T9
	V10 T10
	V11 T11
}

type Tuple13[T0, T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12 any] struct {
	V0  T0
	V1  T1
	V2  T2
	V3  T3
	V4  T4
	V5  T5
	V6  T6
	V7  T7
	V8  T8
	V9  T9
	V10 T10
	V11 T11
	V12 T12
}

type Tuple14[T0, T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12, T13 any] struct {
	V0  T0
	V1  T1
	V2  T2
	V3  T3
	V4  T4
	V5  T5
	V6  T6
	V7  T7
	V8  T8
	V9  T9
	V10 T10
	V11 T11
	V12 T12
	V13 T13
}

type Tuple15[T0, T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12, T13, T14 any] struct {
	V0  T0
	V1  T1
	V2  T2
	V3  T3
	V4  T4
	V5  T5
	V6  T6
	V7  T7
	V8  T8
	V9  T9
	V10 T10
	V11 T11
	V12 T12
	V13 T13
	V14 T14
}

type Tuple16[T0, T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12, T13, T14, T15 any] struct {
	V0  T0
	V1  T1
	V2  T2
	V3  T3
	V4  T4
	V5  T5
	V6  T6
	V7  T7
	V8  T8
	V9  T9
	V10 T10
	V11 T11
	V12 T12
	V13 T13
	V14 T14
	V15 T15
}

type Result[Ok, Err any] struct {
	Ok  *Ok
	Err *Err
}

func Ok[Err, Ok any](v Ok) *Result[Ok, Err] {
	return &Result[Ok, Err]{Ok: &v}
}

func Err[Ok, Err any](v Err) *Result[Ok, Err] {
	return &Result[Ok, Err]{Err: &v}
}

func (v *Result[Ok, Err]) WriteTo(w ByteWriter, fOk func(*Ok, ByteWriter) error, fErr func(*Err, ByteWriter) error) error {
	switch {
	case v.Ok == nil && v.Err == nil:
		return errors.New("both result variants cannot be nil")
	case v.Ok != nil && v.Err != nil:
		return errors.New("exactly one result variant must non-nil")
	case v.Ok != nil:
		slog.Debug("writing `result::ok` status byte")
		if err := w.WriteByte(0); err != nil {
			return fmt.Errorf("failed to write `result::ok` status byte: %w", err)
		}
		slog.Debug("writing `result::ok` payload")
		if err := fOk(v.Ok, w); err != nil {
			return fmt.Errorf("failed to write `result::ok` payload: %w", err)
		}
		return nil
	default:
		slog.Debug("writing `result::err` status byte")
		if err := w.WriteByte(1); err != nil {
			return fmt.Errorf("failed to write `result::err` status byte: %w", err)
		}
		slog.Debug("writing `result::err` payload")
		if err := fErr(v.Err, w); err != nil {
			return fmt.Errorf("failed to write `result::err` payload: %w", err)
		}
		return nil
	}
}

type ByteStreamWriter struct {
	r     io.Reader
	chunk []byte
}

func (v *ByteStreamWriter) WriteTo(w ByteWriter) error {
	if len(v.chunk) == 0 {
		v.chunk = make([]byte, 8096)
	}
	for {
		var end bool
		slog.Debug("reading pending byte stream contents")
		n, err := v.r.Read(v.chunk)
		if err == io.EOF {
			end = true
			slog.Debug("pending byte stream reached EOF")
		} else if err != nil {
			return fmt.Errorf("failed to read pending byte stream chunk: %w", err)
		}
		if n > math.MaxUint32 {
			return fmt.Errorf("pending byte stream chunk length of %d overflows a 32-bit integer", n)
		}
		slog.Debug("writing pending byte stream chunk length", "len", n)
		if err := WriteUint32(uint32(n), w); err != nil {
			return fmt.Errorf("failed to write pending byte stream chunk length of %d: %w", n, err)
		}
		_, err = w.Write(v.chunk[:n])
		if err != nil {
			return fmt.Errorf("failed to write pending byte stream chunk contents: %w", err)
		}
		if end {
			if err := w.WriteByte(0); err != nil {
				return fmt.Errorf("failed to write pending byte stream end byte: %w", err)
			}
			return nil
		}
	}
}

type ByteWriter interface {
	io.ByteWriter
	io.Writer
}

type ByteReader interface {
	io.ByteReader
	io.Reader
}

type Ready interface {
	Ready() bool
}

type Receiver[T any] interface {
	Receive() (T, error)
}

type ReadyReceiver[T any] interface {
	Receiver[T]
	Ready
}

type ReadyReader interface {
	io.Reader
	Ready
}

type ReadyByteReader interface {
	ByteReader
	Ready
}

type byteReader struct {
	*bytes.Reader
}

func (*byteReader) Ready() bool {
	return true
}

type PendingByteReader struct {
	ByteReader
}

func (*PendingByteReader) Ready() bool {
	return false
}

func NewPendingByteReader(r ByteReader) *PendingByteReader {
	return &PendingByteReader{r}
}

type ready[T any] struct {
	v T
}

func (r *ready[T]) Receive() (T, error) {
	return r.v, nil
}

func (*ready[T]) Ready() bool {
	return true
}

type byteStreamReceiver struct {
	ByteReader
	buffered uint32
}

func (r *byteStreamReceiver) Read(p []byte) (int, error) {
	n := r.buffered
	if n == 0 {
		slog.Debug("reading pending byte stream chunk length")
		var err error
		n, err = ReadUint32(r)
		if err != nil {
			return 0, fmt.Errorf("failed to read pending byte stream chunk length: %w", err)
		}
		if n == 0 {
			return 0, io.EOF
		}
	}
	if len(p) > int(n) {
		p = p[:n]
	}
	slog.Debug("reading pending byte stream chunk contents", "len", n)
	rn, err := r.Read(p)
	if err != nil {
		return rn, fmt.Errorf("failed to read pending stream chunk bytes: %w", err)
	}
	if rn > int(n) {
		return rn, errors.New("read more bytes than requested")
	}
	r.buffered = n - uint32(rn)
	return rn, nil
}

func (*byteStreamReceiver) Ready() bool {
	return false
}

type decodeReceiver[T any] struct {
	r      ByteReader
	decode func(ByteReader) (T, error)
}

func (r *decodeReceiver[T]) Receive() (T, error) {
	return r.decode(r.r)
}

func (*decodeReceiver[T]) Ready() bool {
	return false
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

func WriteByteList(v []byte, w ByteWriter) error {
	n := len(v)
	if n > math.MaxUint32 {
		return fmt.Errorf("byte list length of %d overflows a 32-bit integer", n)
	}
	slog.Debug("writing byte list length", "len", n)
	if err := WriteUint32(uint32(n), w); err != nil {
		return fmt.Errorf("failed to write list length of %d: %w", n, err)
	}
	slog.Debug("writing byte list contents")
	_, err := w.Write(v)
	if err != nil {
		return fmt.Errorf("failed to write byte list contents: %w", err)
	}
	return nil
}

func WriteList[T any](v []T, w ByteWriter, f func(T, ByteWriter) error) error {
	n := len(v)
	if n > math.MaxUint32 {
		return fmt.Errorf("list length of %d overflows a 32-bit integer", n)
	}
	slog.Debug("writing list length", "len", n)
	if err := WriteUint32(uint32(n), w); err != nil {
		return fmt.Errorf("failed to write list length of %d: %w", n, err)
	}
	for i := range v {
		slog.Debug("writing list element", "index", i)
		if err := f(v[i], w); err != nil {
			return fmt.Errorf("failed to write list element %d: %w", i, err)
		}
	}
	return nil
}

func WriteOption[T any](v *T, w ByteWriter, f func(T, ByteWriter) error) error {
	if v == nil {
		slog.Debug("writing `option::none` status byte")
		if err := w.WriteByte(0); err != nil {
			return fmt.Errorf("failed to write `option::none` byte: %w", err)
		}
		return nil
	}
	slog.Debug("writing `option::some` status byte")
	if err := w.WriteByte(1); err != nil {
		return fmt.Errorf("failed to write `option::some` status byte: %w", err)
	}
	slog.Debug("writing `option::some` payload")
	if err := f(*v, w); err != nil {
		return fmt.Errorf("failed to write `option::some` payload: %w", err)
	}
	return nil
}

func WriteByteStream(r ReadyReader, w ByteWriter, chunk []byte) (*ByteStreamWriter, error) {
	if r.Ready() {
		slog.Debug("writing byte stream `stream::ready` status byte")
		if err := w.WriteByte(1); err != nil {
			return nil, fmt.Errorf("failed to write `stream::ready` byte: %w", err)
		}
		var buf bytes.Buffer
		slog.Debug("reading ready byte stream contents")
		n, err := io.CopyBuffer(&buf, r, chunk)
		if err != nil {
			return nil, fmt.Errorf("failed to read ready byte stream contents: %w", err)
		}
		slog.Debug("writing ready byte stream contents", "len", n)
		return nil, WriteByteList(buf.Bytes(), w)
	}
	slog.Debug("writing byte stream `stream::pending` status byte")
	if err := w.WriteByte(0); err != nil {
		return nil, fmt.Errorf("failed to write `stream::pending` byte: %w", err)
	}
	return &ByteStreamWriter{r, chunk}, nil
}

// ReadUint64 reads an encoded uint64 from r and returns it.
// The error is [io.EOF] only if no bytes were read.
// If an [io.EOF] happens after reading some but not all the bytes,
// ReadUvarint returns [io.ErrUnexpectedEOF].
func ReadUint64(r ByteReader) (uint64, error) {
	return binary.ReadUvarint(r)
}

// ReadOptionStatus reads a single byte from `r` and returns:
// - `true` for `option::some`
// - `false` for `option::none`
func ReadOptionStatus(r ByteReader) (bool, error) {
	status, err := r.ReadByte()
	if err != nil {
		return false, fmt.Errorf("failed to read `option` status byte: %w", err)
	}
	switch status {
	case 0:
		return false, nil
	case 1:
		return true, nil
	default:
		return false, fmt.Errorf("invalid `option` status byte %d", status)
	}
}

// ReadOption reads an option from `r`
func ReadOption[T any](r ByteReader, f func(ByteReader) (T, error)) (*T, error) {
	slog.Debug("reading option status byte")
	ok, err := ReadOptionStatus(r)
	if err != nil {
		return nil, err
	}
	if !ok {
		return nil, nil
	}
	slog.Debug("reading `option::some` payload")
	v, err := f(r)
	if err != nil {
		return nil, fmt.Errorf("failed to read `option::some` value: %w", err)
	}
	return &v, nil
}

// ReadFlatOption reads an option from `r` without pointer indirection
func ReadFlatOption[T any](r ByteReader, f func(ByteReader) (T, error)) (v T, err error) {
	slog.Debug("reading option status byte")
	ok, err := ReadOptionStatus(r)
	if err != nil {
		return v, err
	}
	if !ok {
		return v, err
	}
	slog.Debug("reading `option::some` payload")
	v, err = f(r)
	if err != nil {
		return v, fmt.Errorf("failed to read `option::some` value: %w", err)
	}
	return v, nil
}

func FlattenOption[T any](v **T) *T {
	if v == nil {
		return nil
	} else {
		return *v
	}
}

// ReadResultStatus reads a single byte from `r` and returns:
// - `true` for `result::ok`
// - `false` for `result::err`
func ReadResultStatus(r ByteReader) (bool, error) {
	status, err := r.ReadByte()
	if err != nil {
		return false, fmt.Errorf("failed to read `result` status byte: %w", err)
	}
	switch status {
	case 0:
		return true, nil
	case 1:
		return false, nil
	default:
		return false, fmt.Errorf("invalid `result` status byte %d", status)
	}
}

// ReadResult reads a single byte from `r`
func ReadResult[T, U any](r ByteReader, fOk func(ByteReader) (T, error), fErr func(ByteReader) (U, error)) (*Result[T, U], error) {
	ok, err := ReadResultStatus(r)
	if err != nil {
		return nil, err
	}
	if !ok {
		v, err := fErr(r)
		if err != nil {
			return nil, fmt.Errorf("failed to read `result::err` value: %w", err)
		}
		return &Result[T, U]{Err: &v}, nil
	}
	v, err := fOk(r)
	if err != nil {
		return nil, fmt.Errorf("failed to read `result::ok` value: %w", err)
	}
	return &Result[T, U]{Ok: &v}, nil
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

// ReadByteList reads a []byte from `r` and returns it
func ReadByteList(r ByteReader) ([]byte, error) {
	slog.Debug("reading byte list length")
	n, err := ReadUint32(r)
	if err != nil {
		return nil, fmt.Errorf("failed to read list length: %w", err)
	}

	b := make([]byte, n)
	slog.Debug("reading bytes", "len", n)
	rn, err := r.Read(b)
	if err != nil {
		return nil, fmt.Errorf("failed to read list bytes: %w", err)
	}
	if rn > int(n) {
		return nil, fmt.Errorf("invalid amount of list bytes read, expected %d, got %d", n, rn)
	}
	return b, nil
}

// ReadList reads a list from `r` and returns it
func ReadList[T any](r ByteReader, f func(ByteReader) (T, error)) ([]T, error) {
	slog.Debug("reading list length")
	n, err := ReadUint32(r)
	if err != nil {
		return nil, fmt.Errorf("failed to read list length: %w", err)
	}
	vs := make([]T, n)
	slog.Debug("reading list elements", "len", n)
	for i := range vs {
		slog.Debug("reading list element", "index", i)
		v, err := f(r)
		if err != nil {
			return nil, fmt.Errorf("failed to read list element %d: %w", i, err)
		}
		vs[i] = v
	}
	return vs, nil
}

func ReadTuple2[T0, T1 any](r ByteReader, f0 func(ByteReader) (T0, error), f1 func(ByteReader) (T1, error)) (*Tuple2[T0, T1], error,
) {
	v0, err := f0(r)
	if err != nil {
		return nil, fmt.Errorf("failed to read tuple element 0: %w", err)
	}
	v1, err := f1(r)
	if err != nil {
		return nil, fmt.Errorf("failed to read tuple element 1: %w", err)
	}
	return &Tuple2[T0, T1]{v0, v1}, nil
}

func ReadTuple3[T0, T1, T2 any](r ByteReader, f0 func(ByteReader) (T0, error), f1 func(ByteReader) (T1, error), f2 func(ByteReader) (T2, error)) (*Tuple3[T0, T1, T2], error,
) {
	v0, err := f0(r)
	if err != nil {
		return nil, fmt.Errorf("failed to read tuple element 0: %w", err)
	}
	v1, err := f1(r)
	if err != nil {
		return nil, fmt.Errorf("failed to read tuple element 1: %w", err)
	}
	v2, err := f2(r)
	if err != nil {
		return nil, fmt.Errorf("failed to read tuple element 2: %w", err)
	}
	return &Tuple3[T0, T1, T2]{v0, v1, v2}, nil
}

// ReadStreamStatus reads a single byte from `r` and returns:
// - `true` if stream is "ready"
// - `false` if stream is "pending"
func ReadStreamStatus(r ByteReader) (bool, error) {
	status, err := r.ReadByte()
	if err != nil {
		return false, fmt.Errorf("failed to read `stream` status byte: %w", err)
	}
	switch status {
	case 0:
		return false, nil
	case 1:
		return true, nil
	default:
		return false, fmt.Errorf("invalid `stream` status byte %d", status)
	}
}

// ReadByteStream reads a stream of bytes from `r` and `ch`
func ReadByteStream(ctx context.Context, r ByteReader, ch <-chan []byte) (ReadyReader, error) {
	slog.DebugContext(ctx, "reading byte stream status byte")
	ok, err := ReadStreamStatus(r)
	if err != nil {
		return nil, err
	}
	if !ok {
		return &byteStreamReceiver{&ChanReader{ctx, ch, nil}, 0}, nil
	}
	slog.DebugContext(ctx, "reading ready byte stream")
	buf, err := ReadByteList(r)
	if err != nil {
		return nil, fmt.Errorf("failed to read bytes: %w", err)
	}
	slog.DebugContext(ctx, "read ready byte stream", "len", len(buf))
	return &byteReader{bytes.NewReader(buf)}, nil
}

// ReadStream reads a stream from `r` and `ch`
func ReadStream[T any](ctx context.Context, r ByteReader, ch <-chan []byte, f func(ByteReader) (T, error)) (ReadyReceiver[[]T], error) {
	slog.DebugContext(ctx, "reading stream status byte")
	ok, err := ReadStreamStatus(r)
	if err != nil {
		return nil, err
	}
	if !ok {
		return &decodeReceiver[[]T]{&ChanReader{ctx, ch, nil}, func(r ByteReader) ([]T, error) {
			n, err := ReadUint32(r)
			if err != nil {
				return nil, fmt.Errorf("failed to read pending stream chunk length: %w", err)
			}
			if n == 0 {
				return nil, io.EOF
			}
			vs := make([]T, n)
			for i := range vs {
				v, err := f(r)
				if err != nil {
					return nil, fmt.Errorf("failed to read pending stream chunk element %d: %w", i, err)
				}
				vs[i] = v
			}
			return vs, nil
		}}, nil
	}
	slog.DebugContext(ctx, "reading ready stream")
	vs, err := ReadList(r, f)
	if err != nil {
		return nil, fmt.Errorf("failed to read ready stream: %w", err)
	}
	slog.DebugContext(ctx, "read ready stream", "len", len(vs))
	return &ready[[]T]{vs}, nil
}

// ReadFutureStatus reads a single byte from `r` and returns:
// - `true` if future is "ready"
// - `false` if future is "pending"
func ReadFutureStatus(r ByteReader) (bool, error) {
	status, err := r.ReadByte()
	if err != nil {
		return false, fmt.Errorf("failed to read `future` status byte: %w", err)
	}
	switch status {
	case 0:
		return false, nil
	case 1:
		return true, nil
	default:
		return false, fmt.Errorf("invalid `future` status byte %d", status)
	}
}

// ReadFuture reads a future from `r` and `ch`
func ReadFuture[T any](ctx context.Context, r ByteReader, ch <-chan []byte, f func(ByteReader) (T, error)) (ReadyReceiver[T], error) {
	slog.DebugContext(ctx, "reading future status byte")
	ok, err := ReadFutureStatus(r)
	if err != nil {
		return nil, err
	}
	if !ok {
		return &decodeReceiver[T]{&ChanReader{ctx, ch, nil}, f}, nil
	}
	slog.DebugContext(ctx, "reading ready future")
	v, err := f(r)
	if err != nil {
		return nil, err
	}
	return &ready[T]{v}, nil
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
func ReadUint16(r ByteReader) (uint16, error) {
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
func ReadUint32(r ByteReader) (uint32, error) {
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
