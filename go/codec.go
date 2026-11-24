package wrpc

import (
	"context"
	"errors"
	"fmt"
	"io"
	"math"
	"slices"
	"unicode/utf8"
)

const byteStreamReadSize = 1 << 12

// Codec encodes and decodes values of type T along with the asynchronous values nested in them
type Codec[T any] interface {
	// Encode appends the encoding of `v` to `dst` and returns the Transmitter of the values pending in `v`, nil if there are none
	Encode(v T, dst []byte) ([]byte, Transmitter, error)
	// Decode decodes a value from `r` and returns the Receiver of the values pending in it, nil if there are none
	Decode(r ByteReader) (T, Receiver, error)
}

// StringCodec is the Codec of `string` values
type StringCodec struct{}

func (StringCodec) Encode(v string, dst []byte) ([]byte, Transmitter, error) {
	if !utf8.ValidString(v) {
		return nil, nil, errors.New("string is not valid UTF-8")
	}
	if uint64(len(v)) > math.MaxUint32 {
		return nil, nil, errors.New("string length overflows uint32")
	}
	return append(AppendUint32(dst, uint32(len(v))), v...), nil, nil
}

func (StringCodec) Decode(r ByteReader) (string, Receiver, error) {
	v, err := ReadString(r)
	return v, nil, err
}

// UnitCodec is the Codec of values with an empty encoding
type UnitCodec struct{}

func (UnitCodec) Encode(_ struct{}, dst []byte) ([]byte, Transmitter, error) {
	return dst, nil, nil
}

func (UnitCodec) Decode(ByteReader) (struct{}, Receiver, error) {
	return struct{}{}, nil, nil
}

// ListCodec is the Codec of `list<T>` values
type ListCodec[T any] struct {
	Elem Codec[T]
}

func (c ListCodec[T]) Encode(v []T, dst []byte) ([]byte, Transmitter, error) {
	dst, ts, err := encodeElements(v, c.Elem, 0, dst)
	if err != nil || len(ts) == 0 {
		return dst, nil, err
	}
	return dst, ts, nil
}

func (c ListCodec[T]) Decode(r ByteReader) ([]T, Receiver, error) {
	vs, rs, err := decodeElements(r, c.Elem, 0, Receivers{})
	if err != nil || len(rs) == 0 {
		return vs, nil, err
	}
	return vs, rs, nil
}

// encodeElements appends the encoding of list `v` using `c` to `dst`, returning the Transmitters of its elements at their index plus `offset`
func encodeElements[T any](v []T, c Codec[T], offset uint32, dst []byte) ([]byte, Transmitters, error) {
	if uint64(len(v)) > math.MaxUint32-uint64(offset) {
		return nil, nil, errors.New("element index overflows uint32")
	}
	return encodeItems(v, c, offset, AppendUint32(dst, uint32(len(v))))
}

// encodeItems appends the encodings of `v` using `c` to `dst`, returning the Transmitters of its elements at their index plus `offset`
func encodeItems[T any](v []T, c Codec[T], offset uint32, dst []byte) ([]byte, Transmitters, error) {
	ts := Transmitters{}
	for i, v := range v {
		var tx Transmitter
		var err error
		if dst, tx, err = c.Encode(v, dst); err != nil {
			return nil, nil, fmt.Errorf("failed to encode element %d: %w", i, err)
		}
		if tx != nil {
			ts[offset+uint32(i)] = tx
		}
	}
	return dst, ts, nil
}

// decodeElements decodes a list using `c`, registering the Receivers of its elements in `rs` at their index plus `offset`
func decodeElements[T any](r ByteReader, c Codec[T], offset uint32, rs Receivers) ([]T, Receivers, error) {
	n, err := ReadUint32(r)
	if err != nil {
		return nil, nil, err
	}
	if math.MaxUint32-n < offset {
		return nil, nil, errors.New("element index overflows uint32")
	}
	vs := make([]T, 0, min(n, 1<<10))
	for i := range n {
		v, rx, err := c.Decode(r)
		if err != nil {
			return nil, nil, fmt.Errorf("failed to decode element %d: %w", i, unexpectedEOF(err))
		}
		if rx != nil {
			rs[offset+i] = rx
		}
		vs = append(vs, v)
	}
	return vs, rs, nil
}

// EncodeFixedList appends the encoding of `list<T, N>` value `v` of length N to `dst` using `c`
func EncodeFixedList[T any](c Codec[T], v []T, dst []byte) ([]byte, Transmitter, error) {
	dst, ts, err := encodeItems(v, c, 0, dst)
	if err != nil || len(ts) == 0 {
		return dst, nil, err
	}
	return dst, ts, nil
}

// DecodeFixedList decodes a `list<T, N>` value into `v` of length N using `c`
func DecodeFixedList[T any](c Codec[T], r ByteReader, v []T) (Receiver, error) {
	rs := Receivers{}
	for i := range v {
		x, rx, err := c.Decode(r)
		if err != nil {
			if i > 0 {
				err = unexpectedEOF(err)
			}
			return nil, fmt.Errorf("failed to decode element %d: %w", i, err)
		}
		if rx != nil {
			rs[uint32(i)] = rx
		}
		v[i] = x
	}
	if len(rs) == 0 {
		return nil, nil
	}
	return rs, nil
}

// MapCodec is the Codec of `map<K, V>` values, decoding fails on duplicate keys
type MapCodec[K comparable, V any] struct {
	Key   Codec[K]
	Value Codec[V]
}

func (c MapCodec[K, V]) Encode(v map[K]V, dst []byte) ([]byte, Transmitter, error) {
	if uint64(len(v)) > math.MaxUint32 {
		return nil, nil, errors.New("map length overflows uint32")
	}
	dst = AppendUint32(dst, uint32(len(v)))
	ts := Transmitters{}
	var i uint32
	for k, v := range v {
		var ktx, vtx Transmitter
		var err error
		if dst, ktx, err = c.Key.Encode(k, dst); err != nil {
			return nil, nil, fmt.Errorf("failed to encode key %d: %w", i, err)
		}
		if dst, vtx, err = c.Value.Encode(v, dst); err != nil {
			return nil, nil, fmt.Errorf("failed to encode value %d: %w", i, err)
		}
		if tx := NewTransmitters(ktx, vtx); tx != nil {
			ts[i] = tx
		}
		i++
	}
	if len(ts) == 0 {
		return dst, nil, nil
	}
	return dst, ts, nil
}

func (c MapCodec[K, V]) Decode(r ByteReader) (map[K]V, Receiver, error) {
	n, err := ReadUint32(r)
	if err != nil {
		return nil, nil, err
	}
	m := make(map[K]V, min(n, 1<<10))
	rs := Receivers{}
	for i := range n {
		k, krx, err := c.Key.Decode(r)
		if err != nil {
			return nil, nil, fmt.Errorf("failed to decode key %d: %w", i, unexpectedEOF(err))
		}
		if _, ok := m[k]; ok {
			return nil, nil, fmt.Errorf("duplicate key at entry %d", i)
		}
		v, vrx, err := c.Value.Decode(r)
		if err != nil {
			return nil, nil, fmt.Errorf("failed to decode value %d: %w", i, unexpectedEOF(err))
		}
		m[k] = v
		if rx := NewReceivers(krx, vrx); rx != nil {
			rs[i] = rx
		}
	}
	if len(rs) == 0 {
		return m, nil, nil
	}
	return m, rs, nil
}

// OptionCodec is the Codec of `option<T>` values
type OptionCodec[T any] struct {
	Elem Codec[T]
}

func (c OptionCodec[T]) Encode(v *T, dst []byte) ([]byte, Transmitter, error) {
	if v == nil {
		return append(dst, 0), nil, nil
	}
	return c.Elem.Encode(*v, append(dst, 1))
}

func (c OptionCodec[T]) Decode(r ByteReader) (*T, Receiver, error) {
	ok, err := readStatus(r)
	if err != nil || !ok {
		return nil, nil, err
	}
	v, rx, err := c.Elem.Decode(r)
	if err != nil {
		return nil, nil, unexpectedEOF(err)
	}
	return &v, rx, nil
}

// PtrCodec is the Codec of pointers to values of type T, encoding fails on nil
type PtrCodec[T any] struct {
	Elem Codec[T]
}

func (c PtrCodec[T]) Encode(v *T, dst []byte) ([]byte, Transmitter, error) {
	if v == nil {
		return nil, nil, errors.New("pointer is nil")
	}
	return c.Elem.Encode(*v, dst)
}

func (c PtrCodec[T]) Decode(r ByteReader) (*T, Receiver, error) {
	v, rx, err := c.Elem.Decode(r)
	if err != nil {
		return nil, nil, err
	}
	return &v, rx, nil
}

// ResultCodec is the Codec of `result<O, E>` values, `UnitCodec` encodes an omitted type
type ResultCodec[O, E any] struct {
	Ok  Codec[O]
	Err Codec[E]
}

func (c ResultCodec[O, E]) Encode(v Result[O, E], dst []byte) ([]byte, Transmitter, error) {
	switch {
	case (v.Ok == nil) == (v.Err == nil):
		return nil, nil, errors.New("exactly one of `ok` and `err` must be set")
	case v.Ok != nil:
		return c.Ok.Encode(*v.Ok, append(dst, 0))
	default:
		return c.Err.Encode(*v.Err, append(dst, 1))
	}
}

func (c ResultCodec[O, E]) Decode(r ByteReader) (Result[O, E], Receiver, error) {
	isErr, err := readStatus(r)
	if err != nil {
		return Result[O, E]{}, nil, err
	}
	if isErr {
		v, rx, err := c.Err.Decode(r)
		if err != nil {
			return Result[O, E]{}, nil, unexpectedEOF(err)
		}
		return Result[O, E]{Err: &v}, rx, nil
	}
	v, rx, err := c.Ok.Decode(r)
	if err != nil {
		return Result[O, E]{}, nil, unexpectedEOF(err)
	}
	return Result[O, E]{Ok: &v}, rx, nil
}

func readStatus(r ByteReader) (bool, error) {
	b, err := r.ReadByte()
	if err != nil {
		return false, err
	}
	switch b {
	case 0:
		return false, nil
	case 1:
		return true, nil
	default:
		return false, fmt.Errorf("invalid status byte `%d`", b)
	}
}

// FutureCodec is the Codec of `future<T>` values, which are delivered once and closed afterwards.
// The bare `future` is `FutureCodec[struct{}]{UnitCodec{}}`.
type FutureCodec[T any] struct {
	Elem Codec[T]
}

func (c FutureCodec[T]) Encode(v <-chan T, dst []byte) ([]byte, Transmitter, error) {
	if v == nil {
		return nil, nil, errors.New("future channel is nil")
	}
	select {
	case x, ok := <-v:
		if !ok {
			return nil, nil, errors.New("future closed without a value")
		}
		dst, tx, err := c.Elem.Encode(x, append(dst, 1))
		if err != nil || tx == nil {
			return dst, nil, err
		}
		return dst, Transmitters{0: tx}, nil
	default:
		return append(dst, 0), futureTransmitter[T]{c.Elem, v}, nil
	}
}

func (c FutureCodec[T]) Decode(r ByteReader) (<-chan T, Receiver, error) {
	ready, err := readStatus(r)
	if err != nil {
		return nil, nil, err
	}
	ch := make(chan T, 1)
	if ready {
		v, rx, err := c.Elem.Decode(r)
		if err != nil {
			return nil, nil, unexpectedEOF(err)
		}
		ch <- v
		close(ch)
		if rx == nil {
			return ch, nil, nil
		}
		return ch, Receivers{0: rx}, nil
	}
	return ch, newPending(func(ctx context.Context, r ByteReader, nested Receivers) error {
		v, rx, err := c.Elem.Decode(r)
		if errors.Is(err, io.EOF) && !errors.Is(err, io.ErrUnexpectedEOF) {
			return errClosedNoValue
		}
		if err != nil {
			return err
		}
		if rx != nil {
			nested[0] = rx
		}
		ch <- v
		close(ch)
		if _, err := r.ReadByte(); err != io.EOF {
			if err == nil {
				return errDataAfterEnd
			}
			return err
		}
		return nil
	}), nil
}

type futureTransmitter[T any] struct {
	elem Codec[T]
	v    <-chan T
}

func (t futureTransmitter[T]) Transmit(ctx context.Context, w *FrameWriter, path []uint32) error {
	var x T
	select {
	case v, ok := <-t.v:
		if !ok {
			return errors.New("future closed without a value")
		}
		x = v
	case <-ctx.Done():
		return context.Cause(ctx)
	}
	b, tx, err := t.elem.Encode(x, nil)
	if err != nil {
		return err
	}
	if len(b) > 0 {
		if err := w.WriteFrame(path, b); err != nil {
			return err
		}
	}
	if err := w.WriteFrame(path, nil); err != nil {
		return err
	}
	if tx == nil {
		return nil
	}
	return tx.Transmit(ctx, w, append(slices.Clip(path), 0))
}

// StreamCodec is the Codec of `stream<T>` values, received chunks are closed only once the stream ended.
// The bare `stream` is `StreamCodec[struct{}]{UnitCodec{}}`.
type StreamCodec[T any] struct {
	Elem Codec[T]
}

func (c StreamCodec[T]) Encode(v <-chan []T, dst []byte) ([]byte, Transmitter, error) {
	if v == nil {
		return nil, nil, errors.New("stream channel is nil")
	}
	return append(dst, 0), streamTransmitter[T]{c.Elem, v}, nil
}

func (c StreamCodec[T]) Decode(r ByteReader) (<-chan []T, Receiver, error) {
	ready, err := readStatus(r)
	if err != nil {
		return nil, nil, err
	}
	ch := make(chan []T, 1)
	if ready {
		vs, rs, err := decodeElements(r, c.Elem, 0, Receivers{})
		if err != nil {
			return nil, nil, unexpectedEOF(err)
		}
		if len(vs) > 0 {
			ch <- vs
		}
		close(ch)
		if len(rs) == 0 {
			return ch, nil, nil
		}
		return ch, rs, nil
	}
	return ch, newPending(func(ctx context.Context, r ByteReader, nested Receivers) error {
		var offset uint32
		for {
			vs, _, err := decodeElements(r, c.Elem, offset, nested)
			if err == io.EOF {
				close(ch)
				return nil
			}
			if err != nil {
				return unexpectedEOF(err)
			}
			offset += uint32(len(vs))
			if len(vs) == 0 {
				continue
			}
			select {
			case ch <- vs:
			case <-ctx.Done():
				return context.Cause(ctx)
			}
		}
	}), nil
}

type streamTransmitter[T any] struct {
	elem Codec[T]
	v    <-chan []T
}

func (t streamTransmitter[T]) Transmit(ctx context.Context, w *FrameWriter, path []uint32) error {
	ctx, cancel := context.WithCancel(ctx)
	defer cancel()
	errs := make(chan error)
	var n int
	var offset uint32
	for {
		select {
		case vs, ok := <-t.v:
			if !ok {
				if err := w.WriteFrame(path, nil); err != nil {
					return err
				}
				for range n {
					select {
					case err := <-errs:
						if err != nil {
							return err
						}
					case <-ctx.Done():
						return context.Cause(ctx)
					}
				}
				return nil
			}
			if len(vs) == 0 {
				continue
			}
			b, ts, err := encodeElements(vs, t.elem, offset, nil)
			if err != nil {
				return err
			}
			offset += uint32(len(vs))
			if err := w.WriteFrame(path, b); err != nil {
				return err
			}
			if len(ts) > 0 {
				n++
				go func() {
					err := ts.Transmit(ctx, w, path)
					select {
					case errs <- err:
					case <-ctx.Done():
					}
				}()
			}
		case err := <-errs:
			n--
			if err != nil {
				return err
			}
		case <-ctx.Done():
			return context.Cause(ctx)
		}
	}
}

// ByteStreamCodec is the Codec of `stream<u8>` values, received chunks are closed only once the stream ended
type ByteStreamCodec struct{}

func (ByteStreamCodec) Encode(v <-chan []byte, dst []byte) ([]byte, Transmitter, error) {
	if v == nil {
		return nil, nil, errors.New("stream channel is nil")
	}
	return append(dst, 0), byteStreamTransmitter(v), nil
}

func (ByteStreamCodec) Decode(r ByteReader) (<-chan []byte, Receiver, error) {
	ready, err := readStatus(r)
	if err != nil {
		return nil, nil, err
	}
	ch := make(chan []byte, 1)
	if ready {
		b, err := ReadByteList(r)
		if err != nil {
			return nil, nil, unexpectedEOF(err)
		}
		if len(b) > 0 {
			ch <- b
		}
		close(ch)
		return ch, nil, nil
	}
	return ch, newPending(func(ctx context.Context, r ByteReader, _ Receivers) error {
		for {
			n, err := ReadUint32(r)
			if err == io.EOF {
				close(ch)
				return nil
			}
			if err != nil {
				return err
			}
			for n > 0 {
				b := make([]byte, min(n, byteStreamReadSize))
				k, err := r.Read(b)
				if err != nil {
					return unexpectedEOF(err)
				}
				n -= uint32(k)
				select {
				case ch <- b[:k]:
				case <-ctx.Done():
					return context.Cause(ctx)
				}
			}
		}
	}), nil
}

type byteStreamTransmitter <-chan []byte

func (t byteStreamTransmitter) Transmit(ctx context.Context, w *FrameWriter, path []uint32) error {
	for {
		select {
		case b, ok := <-t:
			if !ok {
				return w.WriteFrame(path, nil)
			}
			if len(b) == 0 {
				continue
			}
			if uint64(len(b)) > math.MaxUint32 {
				return errors.New("stream chunk length overflows uint32")
			}
			if err := w.WriteFrame(path, append(AppendUint32(nil, uint32(len(b))), b...)); err != nil {
				return err
			}
		case <-ctx.Done():
			return context.Cause(ctx)
		}
	}
}
