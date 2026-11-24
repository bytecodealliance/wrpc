package wrpc

import "fmt"

type tupleEncoder struct {
	dst []byte
	txs []Transmitter
	err error
}

func encodeMember[T any](e *tupleEncoder, c Codec[T], v T) {
	if e.err != nil {
		return
	}
	var tx Transmitter
	var err error
	if e.dst, tx, err = c.Encode(v, e.dst); err != nil {
		e.err = fmt.Errorf("failed to encode member %d: %w", len(e.txs), err)
		return
	}
	e.txs = append(e.txs, tx)
}

func (e *tupleEncoder) result() ([]byte, Transmitter, error) {
	if e.err != nil {
		return nil, nil, e.err
	}
	return e.dst, NewTransmitters(e.txs...), nil
}

type tupleDecoder struct {
	r   ByteReader
	rxs []Receiver
	err error
}

func decodeMember[T any](d *tupleDecoder, c Codec[T], v *T) {
	if d.err != nil {
		return
	}
	x, rx, err := c.Decode(d.r)
	if err != nil {
		if len(d.rxs) > 0 {
			err = unexpectedEOF(err)
		}
		d.err = fmt.Errorf("failed to decode member %d: %w", len(d.rxs), err)
		return
	}
	*v = x
	d.rxs = append(d.rxs, rx)
}

func (d *tupleDecoder) result() (Receiver, error) {
	if d.err != nil {
		return nil, d.err
	}
	return NewReceivers(d.rxs...), nil
}

// Tuple1Codec is the Codec of `tuple<T0>` values
type Tuple1Codec[T0 any] struct {
	C0 Codec[T0]
}

func (c Tuple1Codec[T0]) Encode(v Tuple1[T0], dst []byte) ([]byte, Transmitter, error) {
	e := tupleEncoder{dst: dst}
	encodeMember(&e, c.C0, v.V0)
	return e.result()
}

func (c Tuple1Codec[T0]) Decode(r ByteReader) (Tuple1[T0], Receiver, error) {
	var v Tuple1[T0]
	d := tupleDecoder{r: r}
	decodeMember(&d, c.C0, &v.V0)
	rx, err := d.result()
	return v, rx, err
}

// Tuple2Codec is the Codec of `tuple<T0, T1>` values
type Tuple2Codec[T0, T1 any] struct {
	C0 Codec[T0]
	C1 Codec[T1]
}

func (c Tuple2Codec[T0, T1]) Encode(v Tuple2[T0, T1], dst []byte) ([]byte, Transmitter, error) {
	e := tupleEncoder{dst: dst}
	encodeMember(&e, c.C0, v.V0)
	encodeMember(&e, c.C1, v.V1)
	return e.result()
}

func (c Tuple2Codec[T0, T1]) Decode(r ByteReader) (Tuple2[T0, T1], Receiver, error) {
	var v Tuple2[T0, T1]
	d := tupleDecoder{r: r}
	decodeMember(&d, c.C0, &v.V0)
	decodeMember(&d, c.C1, &v.V1)
	rx, err := d.result()
	return v, rx, err
}

// Tuple3Codec is the Codec of `tuple<T0, T1, T2>` values
type Tuple3Codec[T0, T1, T2 any] struct {
	C0 Codec[T0]
	C1 Codec[T1]
	C2 Codec[T2]
}

func (c Tuple3Codec[T0, T1, T2]) Encode(v Tuple3[T0, T1, T2], dst []byte) ([]byte, Transmitter, error) {
	e := tupleEncoder{dst: dst}
	encodeMember(&e, c.C0, v.V0)
	encodeMember(&e, c.C1, v.V1)
	encodeMember(&e, c.C2, v.V2)
	return e.result()
}

func (c Tuple3Codec[T0, T1, T2]) Decode(r ByteReader) (Tuple3[T0, T1, T2], Receiver, error) {
	var v Tuple3[T0, T1, T2]
	d := tupleDecoder{r: r}
	decodeMember(&d, c.C0, &v.V0)
	decodeMember(&d, c.C1, &v.V1)
	decodeMember(&d, c.C2, &v.V2)
	rx, err := d.result()
	return v, rx, err
}

// Tuple4Codec is the Codec of `tuple<T0, T1, T2, T3>` values
type Tuple4Codec[T0, T1, T2, T3 any] struct {
	C0 Codec[T0]
	C1 Codec[T1]
	C2 Codec[T2]
	C3 Codec[T3]
}

func (c Tuple4Codec[T0, T1, T2, T3]) Encode(v Tuple4[T0, T1, T2, T3], dst []byte) ([]byte, Transmitter, error) {
	e := tupleEncoder{dst: dst}
	encodeMember(&e, c.C0, v.V0)
	encodeMember(&e, c.C1, v.V1)
	encodeMember(&e, c.C2, v.V2)
	encodeMember(&e, c.C3, v.V3)
	return e.result()
}

func (c Tuple4Codec[T0, T1, T2, T3]) Decode(r ByteReader) (Tuple4[T0, T1, T2, T3], Receiver, error) {
	var v Tuple4[T0, T1, T2, T3]
	d := tupleDecoder{r: r}
	decodeMember(&d, c.C0, &v.V0)
	decodeMember(&d, c.C1, &v.V1)
	decodeMember(&d, c.C2, &v.V2)
	decodeMember(&d, c.C3, &v.V3)
	rx, err := d.result()
	return v, rx, err
}

// Tuple5Codec is the Codec of `tuple<T0, T1, T2, T3, T4>` values
type Tuple5Codec[T0, T1, T2, T3, T4 any] struct {
	C0 Codec[T0]
	C1 Codec[T1]
	C2 Codec[T2]
	C3 Codec[T3]
	C4 Codec[T4]
}

func (c Tuple5Codec[T0, T1, T2, T3, T4]) Encode(v Tuple5[T0, T1, T2, T3, T4], dst []byte) ([]byte, Transmitter, error) {
	e := tupleEncoder{dst: dst}
	encodeMember(&e, c.C0, v.V0)
	encodeMember(&e, c.C1, v.V1)
	encodeMember(&e, c.C2, v.V2)
	encodeMember(&e, c.C3, v.V3)
	encodeMember(&e, c.C4, v.V4)
	return e.result()
}

func (c Tuple5Codec[T0, T1, T2, T3, T4]) Decode(r ByteReader) (Tuple5[T0, T1, T2, T3, T4], Receiver, error) {
	var v Tuple5[T0, T1, T2, T3, T4]
	d := tupleDecoder{r: r}
	decodeMember(&d, c.C0, &v.V0)
	decodeMember(&d, c.C1, &v.V1)
	decodeMember(&d, c.C2, &v.V2)
	decodeMember(&d, c.C3, &v.V3)
	decodeMember(&d, c.C4, &v.V4)
	rx, err := d.result()
	return v, rx, err
}

// Tuple6Codec is the Codec of `tuple<T0, T1, T2, T3, T4, T5>` values
type Tuple6Codec[T0, T1, T2, T3, T4, T5 any] struct {
	C0 Codec[T0]
	C1 Codec[T1]
	C2 Codec[T2]
	C3 Codec[T3]
	C4 Codec[T4]
	C5 Codec[T5]
}

func (c Tuple6Codec[T0, T1, T2, T3, T4, T5]) Encode(v Tuple6[T0, T1, T2, T3, T4, T5], dst []byte) ([]byte, Transmitter, error) {
	e := tupleEncoder{dst: dst}
	encodeMember(&e, c.C0, v.V0)
	encodeMember(&e, c.C1, v.V1)
	encodeMember(&e, c.C2, v.V2)
	encodeMember(&e, c.C3, v.V3)
	encodeMember(&e, c.C4, v.V4)
	encodeMember(&e, c.C5, v.V5)
	return e.result()
}

func (c Tuple6Codec[T0, T1, T2, T3, T4, T5]) Decode(r ByteReader) (Tuple6[T0, T1, T2, T3, T4, T5], Receiver, error) {
	var v Tuple6[T0, T1, T2, T3, T4, T5]
	d := tupleDecoder{r: r}
	decodeMember(&d, c.C0, &v.V0)
	decodeMember(&d, c.C1, &v.V1)
	decodeMember(&d, c.C2, &v.V2)
	decodeMember(&d, c.C3, &v.V3)
	decodeMember(&d, c.C4, &v.V4)
	decodeMember(&d, c.C5, &v.V5)
	rx, err := d.result()
	return v, rx, err
}

// Tuple7Codec is the Codec of `tuple<T0, T1, T2, T3, T4, T5, T6>` values
type Tuple7Codec[T0, T1, T2, T3, T4, T5, T6 any] struct {
	C0 Codec[T0]
	C1 Codec[T1]
	C2 Codec[T2]
	C3 Codec[T3]
	C4 Codec[T4]
	C5 Codec[T5]
	C6 Codec[T6]
}

func (c Tuple7Codec[T0, T1, T2, T3, T4, T5, T6]) Encode(v Tuple7[T0, T1, T2, T3, T4, T5, T6], dst []byte) ([]byte, Transmitter, error) {
	e := tupleEncoder{dst: dst}
	encodeMember(&e, c.C0, v.V0)
	encodeMember(&e, c.C1, v.V1)
	encodeMember(&e, c.C2, v.V2)
	encodeMember(&e, c.C3, v.V3)
	encodeMember(&e, c.C4, v.V4)
	encodeMember(&e, c.C5, v.V5)
	encodeMember(&e, c.C6, v.V6)
	return e.result()
}

func (c Tuple7Codec[T0, T1, T2, T3, T4, T5, T6]) Decode(r ByteReader) (Tuple7[T0, T1, T2, T3, T4, T5, T6], Receiver, error) {
	var v Tuple7[T0, T1, T2, T3, T4, T5, T6]
	d := tupleDecoder{r: r}
	decodeMember(&d, c.C0, &v.V0)
	decodeMember(&d, c.C1, &v.V1)
	decodeMember(&d, c.C2, &v.V2)
	decodeMember(&d, c.C3, &v.V3)
	decodeMember(&d, c.C4, &v.V4)
	decodeMember(&d, c.C5, &v.V5)
	decodeMember(&d, c.C6, &v.V6)
	rx, err := d.result()
	return v, rx, err
}

// Tuple8Codec is the Codec of `tuple<T0, T1, T2, T3, T4, T5, T6, T7>` values
type Tuple8Codec[T0, T1, T2, T3, T4, T5, T6, T7 any] struct {
	C0 Codec[T0]
	C1 Codec[T1]
	C2 Codec[T2]
	C3 Codec[T3]
	C4 Codec[T4]
	C5 Codec[T5]
	C6 Codec[T6]
	C7 Codec[T7]
}

func (c Tuple8Codec[T0, T1, T2, T3, T4, T5, T6, T7]) Encode(v Tuple8[T0, T1, T2, T3, T4, T5, T6, T7], dst []byte) ([]byte, Transmitter, error) {
	e := tupleEncoder{dst: dst}
	encodeMember(&e, c.C0, v.V0)
	encodeMember(&e, c.C1, v.V1)
	encodeMember(&e, c.C2, v.V2)
	encodeMember(&e, c.C3, v.V3)
	encodeMember(&e, c.C4, v.V4)
	encodeMember(&e, c.C5, v.V5)
	encodeMember(&e, c.C6, v.V6)
	encodeMember(&e, c.C7, v.V7)
	return e.result()
}

func (c Tuple8Codec[T0, T1, T2, T3, T4, T5, T6, T7]) Decode(r ByteReader) (Tuple8[T0, T1, T2, T3, T4, T5, T6, T7], Receiver, error) {
	var v Tuple8[T0, T1, T2, T3, T4, T5, T6, T7]
	d := tupleDecoder{r: r}
	decodeMember(&d, c.C0, &v.V0)
	decodeMember(&d, c.C1, &v.V1)
	decodeMember(&d, c.C2, &v.V2)
	decodeMember(&d, c.C3, &v.V3)
	decodeMember(&d, c.C4, &v.V4)
	decodeMember(&d, c.C5, &v.V5)
	decodeMember(&d, c.C6, &v.V6)
	decodeMember(&d, c.C7, &v.V7)
	rx, err := d.result()
	return v, rx, err
}

// Tuple9Codec is the Codec of `tuple<T0, T1, T2, T3, T4, T5, T6, T7, T8>` values
type Tuple9Codec[T0, T1, T2, T3, T4, T5, T6, T7, T8 any] struct {
	C0 Codec[T0]
	C1 Codec[T1]
	C2 Codec[T2]
	C3 Codec[T3]
	C4 Codec[T4]
	C5 Codec[T5]
	C6 Codec[T6]
	C7 Codec[T7]
	C8 Codec[T8]
}

func (c Tuple9Codec[T0, T1, T2, T3, T4, T5, T6, T7, T8]) Encode(v Tuple9[T0, T1, T2, T3, T4, T5, T6, T7, T8], dst []byte) ([]byte, Transmitter, error) {
	e := tupleEncoder{dst: dst}
	encodeMember(&e, c.C0, v.V0)
	encodeMember(&e, c.C1, v.V1)
	encodeMember(&e, c.C2, v.V2)
	encodeMember(&e, c.C3, v.V3)
	encodeMember(&e, c.C4, v.V4)
	encodeMember(&e, c.C5, v.V5)
	encodeMember(&e, c.C6, v.V6)
	encodeMember(&e, c.C7, v.V7)
	encodeMember(&e, c.C8, v.V8)
	return e.result()
}

func (c Tuple9Codec[T0, T1, T2, T3, T4, T5, T6, T7, T8]) Decode(r ByteReader) (Tuple9[T0, T1, T2, T3, T4, T5, T6, T7, T8], Receiver, error) {
	var v Tuple9[T0, T1, T2, T3, T4, T5, T6, T7, T8]
	d := tupleDecoder{r: r}
	decodeMember(&d, c.C0, &v.V0)
	decodeMember(&d, c.C1, &v.V1)
	decodeMember(&d, c.C2, &v.V2)
	decodeMember(&d, c.C3, &v.V3)
	decodeMember(&d, c.C4, &v.V4)
	decodeMember(&d, c.C5, &v.V5)
	decodeMember(&d, c.C6, &v.V6)
	decodeMember(&d, c.C7, &v.V7)
	decodeMember(&d, c.C8, &v.V8)
	rx, err := d.result()
	return v, rx, err
}

// Tuple10Codec is the Codec of `tuple<T0, T1, T2, T3, T4, T5, T6, T7, T8, T9>` values
type Tuple10Codec[T0, T1, T2, T3, T4, T5, T6, T7, T8, T9 any] struct {
	C0 Codec[T0]
	C1 Codec[T1]
	C2 Codec[T2]
	C3 Codec[T3]
	C4 Codec[T4]
	C5 Codec[T5]
	C6 Codec[T6]
	C7 Codec[T7]
	C8 Codec[T8]
	C9 Codec[T9]
}

func (c Tuple10Codec[T0, T1, T2, T3, T4, T5, T6, T7, T8, T9]) Encode(v Tuple10[T0, T1, T2, T3, T4, T5, T6, T7, T8, T9], dst []byte) ([]byte, Transmitter, error) {
	e := tupleEncoder{dst: dst}
	encodeMember(&e, c.C0, v.V0)
	encodeMember(&e, c.C1, v.V1)
	encodeMember(&e, c.C2, v.V2)
	encodeMember(&e, c.C3, v.V3)
	encodeMember(&e, c.C4, v.V4)
	encodeMember(&e, c.C5, v.V5)
	encodeMember(&e, c.C6, v.V6)
	encodeMember(&e, c.C7, v.V7)
	encodeMember(&e, c.C8, v.V8)
	encodeMember(&e, c.C9, v.V9)
	return e.result()
}

func (c Tuple10Codec[T0, T1, T2, T3, T4, T5, T6, T7, T8, T9]) Decode(r ByteReader) (Tuple10[T0, T1, T2, T3, T4, T5, T6, T7, T8, T9], Receiver, error) {
	var v Tuple10[T0, T1, T2, T3, T4, T5, T6, T7, T8, T9]
	d := tupleDecoder{r: r}
	decodeMember(&d, c.C0, &v.V0)
	decodeMember(&d, c.C1, &v.V1)
	decodeMember(&d, c.C2, &v.V2)
	decodeMember(&d, c.C3, &v.V3)
	decodeMember(&d, c.C4, &v.V4)
	decodeMember(&d, c.C5, &v.V5)
	decodeMember(&d, c.C6, &v.V6)
	decodeMember(&d, c.C7, &v.V7)
	decodeMember(&d, c.C8, &v.V8)
	decodeMember(&d, c.C9, &v.V9)
	rx, err := d.result()
	return v, rx, err
}

// Tuple11Codec is the Codec of `tuple<T0, T1, T2, T3, T4, T5, T6, T7, T8, T9, T10>` values
type Tuple11Codec[T0, T1, T2, T3, T4, T5, T6, T7, T8, T9, T10 any] struct {
	C0  Codec[T0]
	C1  Codec[T1]
	C2  Codec[T2]
	C3  Codec[T3]
	C4  Codec[T4]
	C5  Codec[T5]
	C6  Codec[T6]
	C7  Codec[T7]
	C8  Codec[T8]
	C9  Codec[T9]
	C10 Codec[T10]
}

func (c Tuple11Codec[T0, T1, T2, T3, T4, T5, T6, T7, T8, T9, T10]) Encode(v Tuple11[T0, T1, T2, T3, T4, T5, T6, T7, T8, T9, T10], dst []byte) ([]byte, Transmitter, error) {
	e := tupleEncoder{dst: dst}
	encodeMember(&e, c.C0, v.V0)
	encodeMember(&e, c.C1, v.V1)
	encodeMember(&e, c.C2, v.V2)
	encodeMember(&e, c.C3, v.V3)
	encodeMember(&e, c.C4, v.V4)
	encodeMember(&e, c.C5, v.V5)
	encodeMember(&e, c.C6, v.V6)
	encodeMember(&e, c.C7, v.V7)
	encodeMember(&e, c.C8, v.V8)
	encodeMember(&e, c.C9, v.V9)
	encodeMember(&e, c.C10, v.V10)
	return e.result()
}

func (c Tuple11Codec[T0, T1, T2, T3, T4, T5, T6, T7, T8, T9, T10]) Decode(r ByteReader) (Tuple11[T0, T1, T2, T3, T4, T5, T6, T7, T8, T9, T10], Receiver, error) {
	var v Tuple11[T0, T1, T2, T3, T4, T5, T6, T7, T8, T9, T10]
	d := tupleDecoder{r: r}
	decodeMember(&d, c.C0, &v.V0)
	decodeMember(&d, c.C1, &v.V1)
	decodeMember(&d, c.C2, &v.V2)
	decodeMember(&d, c.C3, &v.V3)
	decodeMember(&d, c.C4, &v.V4)
	decodeMember(&d, c.C5, &v.V5)
	decodeMember(&d, c.C6, &v.V6)
	decodeMember(&d, c.C7, &v.V7)
	decodeMember(&d, c.C8, &v.V8)
	decodeMember(&d, c.C9, &v.V9)
	decodeMember(&d, c.C10, &v.V10)
	rx, err := d.result()
	return v, rx, err
}

// Tuple12Codec is the Codec of `tuple<T0, T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11>` values
type Tuple12Codec[T0, T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11 any] struct {
	C0  Codec[T0]
	C1  Codec[T1]
	C2  Codec[T2]
	C3  Codec[T3]
	C4  Codec[T4]
	C5  Codec[T5]
	C6  Codec[T6]
	C7  Codec[T7]
	C8  Codec[T8]
	C9  Codec[T9]
	C10 Codec[T10]
	C11 Codec[T11]
}

func (c Tuple12Codec[T0, T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11]) Encode(v Tuple12[T0, T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11], dst []byte) ([]byte, Transmitter, error) {
	e := tupleEncoder{dst: dst}
	encodeMember(&e, c.C0, v.V0)
	encodeMember(&e, c.C1, v.V1)
	encodeMember(&e, c.C2, v.V2)
	encodeMember(&e, c.C3, v.V3)
	encodeMember(&e, c.C4, v.V4)
	encodeMember(&e, c.C5, v.V5)
	encodeMember(&e, c.C6, v.V6)
	encodeMember(&e, c.C7, v.V7)
	encodeMember(&e, c.C8, v.V8)
	encodeMember(&e, c.C9, v.V9)
	encodeMember(&e, c.C10, v.V10)
	encodeMember(&e, c.C11, v.V11)
	return e.result()
}

func (c Tuple12Codec[T0, T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11]) Decode(r ByteReader) (Tuple12[T0, T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11], Receiver, error) {
	var v Tuple12[T0, T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11]
	d := tupleDecoder{r: r}
	decodeMember(&d, c.C0, &v.V0)
	decodeMember(&d, c.C1, &v.V1)
	decodeMember(&d, c.C2, &v.V2)
	decodeMember(&d, c.C3, &v.V3)
	decodeMember(&d, c.C4, &v.V4)
	decodeMember(&d, c.C5, &v.V5)
	decodeMember(&d, c.C6, &v.V6)
	decodeMember(&d, c.C7, &v.V7)
	decodeMember(&d, c.C8, &v.V8)
	decodeMember(&d, c.C9, &v.V9)
	decodeMember(&d, c.C10, &v.V10)
	decodeMember(&d, c.C11, &v.V11)
	rx, err := d.result()
	return v, rx, err
}

// Tuple13Codec is the Codec of `tuple<T0, T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12>` values
type Tuple13Codec[T0, T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12 any] struct {
	C0  Codec[T0]
	C1  Codec[T1]
	C2  Codec[T2]
	C3  Codec[T3]
	C4  Codec[T4]
	C5  Codec[T5]
	C6  Codec[T6]
	C7  Codec[T7]
	C8  Codec[T8]
	C9  Codec[T9]
	C10 Codec[T10]
	C11 Codec[T11]
	C12 Codec[T12]
}

func (c Tuple13Codec[T0, T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12]) Encode(v Tuple13[T0, T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12], dst []byte) ([]byte, Transmitter, error) {
	e := tupleEncoder{dst: dst}
	encodeMember(&e, c.C0, v.V0)
	encodeMember(&e, c.C1, v.V1)
	encodeMember(&e, c.C2, v.V2)
	encodeMember(&e, c.C3, v.V3)
	encodeMember(&e, c.C4, v.V4)
	encodeMember(&e, c.C5, v.V5)
	encodeMember(&e, c.C6, v.V6)
	encodeMember(&e, c.C7, v.V7)
	encodeMember(&e, c.C8, v.V8)
	encodeMember(&e, c.C9, v.V9)
	encodeMember(&e, c.C10, v.V10)
	encodeMember(&e, c.C11, v.V11)
	encodeMember(&e, c.C12, v.V12)
	return e.result()
}

func (c Tuple13Codec[T0, T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12]) Decode(r ByteReader) (Tuple13[T0, T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12], Receiver, error) {
	var v Tuple13[T0, T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12]
	d := tupleDecoder{r: r}
	decodeMember(&d, c.C0, &v.V0)
	decodeMember(&d, c.C1, &v.V1)
	decodeMember(&d, c.C2, &v.V2)
	decodeMember(&d, c.C3, &v.V3)
	decodeMember(&d, c.C4, &v.V4)
	decodeMember(&d, c.C5, &v.V5)
	decodeMember(&d, c.C6, &v.V6)
	decodeMember(&d, c.C7, &v.V7)
	decodeMember(&d, c.C8, &v.V8)
	decodeMember(&d, c.C9, &v.V9)
	decodeMember(&d, c.C10, &v.V10)
	decodeMember(&d, c.C11, &v.V11)
	decodeMember(&d, c.C12, &v.V12)
	rx, err := d.result()
	return v, rx, err
}

// Tuple14Codec is the Codec of `tuple<T0, T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12, T13>` values
type Tuple14Codec[T0, T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12, T13 any] struct {
	C0  Codec[T0]
	C1  Codec[T1]
	C2  Codec[T2]
	C3  Codec[T3]
	C4  Codec[T4]
	C5  Codec[T5]
	C6  Codec[T6]
	C7  Codec[T7]
	C8  Codec[T8]
	C9  Codec[T9]
	C10 Codec[T10]
	C11 Codec[T11]
	C12 Codec[T12]
	C13 Codec[T13]
}

func (c Tuple14Codec[T0, T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12, T13]) Encode(v Tuple14[T0, T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12, T13], dst []byte) ([]byte, Transmitter, error) {
	e := tupleEncoder{dst: dst}
	encodeMember(&e, c.C0, v.V0)
	encodeMember(&e, c.C1, v.V1)
	encodeMember(&e, c.C2, v.V2)
	encodeMember(&e, c.C3, v.V3)
	encodeMember(&e, c.C4, v.V4)
	encodeMember(&e, c.C5, v.V5)
	encodeMember(&e, c.C6, v.V6)
	encodeMember(&e, c.C7, v.V7)
	encodeMember(&e, c.C8, v.V8)
	encodeMember(&e, c.C9, v.V9)
	encodeMember(&e, c.C10, v.V10)
	encodeMember(&e, c.C11, v.V11)
	encodeMember(&e, c.C12, v.V12)
	encodeMember(&e, c.C13, v.V13)
	return e.result()
}

func (c Tuple14Codec[T0, T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12, T13]) Decode(r ByteReader) (Tuple14[T0, T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12, T13], Receiver, error) {
	var v Tuple14[T0, T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12, T13]
	d := tupleDecoder{r: r}
	decodeMember(&d, c.C0, &v.V0)
	decodeMember(&d, c.C1, &v.V1)
	decodeMember(&d, c.C2, &v.V2)
	decodeMember(&d, c.C3, &v.V3)
	decodeMember(&d, c.C4, &v.V4)
	decodeMember(&d, c.C5, &v.V5)
	decodeMember(&d, c.C6, &v.V6)
	decodeMember(&d, c.C7, &v.V7)
	decodeMember(&d, c.C8, &v.V8)
	decodeMember(&d, c.C9, &v.V9)
	decodeMember(&d, c.C10, &v.V10)
	decodeMember(&d, c.C11, &v.V11)
	decodeMember(&d, c.C12, &v.V12)
	decodeMember(&d, c.C13, &v.V13)
	rx, err := d.result()
	return v, rx, err
}

// Tuple15Codec is the Codec of `tuple<T0, T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12, T13, T14>` values
type Tuple15Codec[T0, T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12, T13, T14 any] struct {
	C0  Codec[T0]
	C1  Codec[T1]
	C2  Codec[T2]
	C3  Codec[T3]
	C4  Codec[T4]
	C5  Codec[T5]
	C6  Codec[T6]
	C7  Codec[T7]
	C8  Codec[T8]
	C9  Codec[T9]
	C10 Codec[T10]
	C11 Codec[T11]
	C12 Codec[T12]
	C13 Codec[T13]
	C14 Codec[T14]
}

func (c Tuple15Codec[T0, T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12, T13, T14]) Encode(v Tuple15[T0, T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12, T13, T14], dst []byte) ([]byte, Transmitter, error) {
	e := tupleEncoder{dst: dst}
	encodeMember(&e, c.C0, v.V0)
	encodeMember(&e, c.C1, v.V1)
	encodeMember(&e, c.C2, v.V2)
	encodeMember(&e, c.C3, v.V3)
	encodeMember(&e, c.C4, v.V4)
	encodeMember(&e, c.C5, v.V5)
	encodeMember(&e, c.C6, v.V6)
	encodeMember(&e, c.C7, v.V7)
	encodeMember(&e, c.C8, v.V8)
	encodeMember(&e, c.C9, v.V9)
	encodeMember(&e, c.C10, v.V10)
	encodeMember(&e, c.C11, v.V11)
	encodeMember(&e, c.C12, v.V12)
	encodeMember(&e, c.C13, v.V13)
	encodeMember(&e, c.C14, v.V14)
	return e.result()
}

func (c Tuple15Codec[T0, T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12, T13, T14]) Decode(r ByteReader) (Tuple15[T0, T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12, T13, T14], Receiver, error) {
	var v Tuple15[T0, T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12, T13, T14]
	d := tupleDecoder{r: r}
	decodeMember(&d, c.C0, &v.V0)
	decodeMember(&d, c.C1, &v.V1)
	decodeMember(&d, c.C2, &v.V2)
	decodeMember(&d, c.C3, &v.V3)
	decodeMember(&d, c.C4, &v.V4)
	decodeMember(&d, c.C5, &v.V5)
	decodeMember(&d, c.C6, &v.V6)
	decodeMember(&d, c.C7, &v.V7)
	decodeMember(&d, c.C8, &v.V8)
	decodeMember(&d, c.C9, &v.V9)
	decodeMember(&d, c.C10, &v.V10)
	decodeMember(&d, c.C11, &v.V11)
	decodeMember(&d, c.C12, &v.V12)
	decodeMember(&d, c.C13, &v.V13)
	decodeMember(&d, c.C14, &v.V14)
	rx, err := d.result()
	return v, rx, err
}

// Tuple16Codec is the Codec of `tuple<T0, T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12, T13, T14, T15>` values
type Tuple16Codec[T0, T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12, T13, T14, T15 any] struct {
	C0  Codec[T0]
	C1  Codec[T1]
	C2  Codec[T2]
	C3  Codec[T3]
	C4  Codec[T4]
	C5  Codec[T5]
	C6  Codec[T6]
	C7  Codec[T7]
	C8  Codec[T8]
	C9  Codec[T9]
	C10 Codec[T10]
	C11 Codec[T11]
	C12 Codec[T12]
	C13 Codec[T13]
	C14 Codec[T14]
	C15 Codec[T15]
}

func (c Tuple16Codec[T0, T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12, T13, T14, T15]) Encode(v Tuple16[T0, T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12, T13, T14, T15], dst []byte) ([]byte, Transmitter, error) {
	e := tupleEncoder{dst: dst}
	encodeMember(&e, c.C0, v.V0)
	encodeMember(&e, c.C1, v.V1)
	encodeMember(&e, c.C2, v.V2)
	encodeMember(&e, c.C3, v.V3)
	encodeMember(&e, c.C4, v.V4)
	encodeMember(&e, c.C5, v.V5)
	encodeMember(&e, c.C6, v.V6)
	encodeMember(&e, c.C7, v.V7)
	encodeMember(&e, c.C8, v.V8)
	encodeMember(&e, c.C9, v.V9)
	encodeMember(&e, c.C10, v.V10)
	encodeMember(&e, c.C11, v.V11)
	encodeMember(&e, c.C12, v.V12)
	encodeMember(&e, c.C13, v.V13)
	encodeMember(&e, c.C14, v.V14)
	encodeMember(&e, c.C15, v.V15)
	return e.result()
}

func (c Tuple16Codec[T0, T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12, T13, T14, T15]) Decode(r ByteReader) (Tuple16[T0, T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12, T13, T14, T15], Receiver, error) {
	var v Tuple16[T0, T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12, T13, T14, T15]
	d := tupleDecoder{r: r}
	decodeMember(&d, c.C0, &v.V0)
	decodeMember(&d, c.C1, &v.V1)
	decodeMember(&d, c.C2, &v.V2)
	decodeMember(&d, c.C3, &v.V3)
	decodeMember(&d, c.C4, &v.V4)
	decodeMember(&d, c.C5, &v.V5)
	decodeMember(&d, c.C6, &v.V6)
	decodeMember(&d, c.C7, &v.V7)
	decodeMember(&d, c.C8, &v.V8)
	decodeMember(&d, c.C9, &v.V9)
	decodeMember(&d, c.C10, &v.V10)
	decodeMember(&d, c.C11, &v.V11)
	decodeMember(&d, c.C12, &v.V12)
	decodeMember(&d, c.C13, &v.V13)
	decodeMember(&d, c.C14, &v.V14)
	decodeMember(&d, c.C15, &v.V15)
	rx, err := d.result()
	return v, rx, err
}
