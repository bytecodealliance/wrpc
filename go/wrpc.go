package wrpc

import (
	"bytes"
	"context"
	"io"
)

type SubscribePath []*uint32

func NewSubscribePath(ps ...*uint32) SubscribePath {
	return SubscribePath(ps)
}

func (p SubscribePath) push(v *uint32) SubscribePath {
	return SubscribePath(append(append(make(SubscribePath, 0, len(p)+1), p...), v))
}

func (p SubscribePath) Index(i uint32) SubscribePath {
	return p.push(&i)
}

func (p SubscribePath) Wildcard() SubscribePath {
	return p.push(nil)
}

func (p SubscribePath) Parent() (SubscribePath, bool) {
	n := len(p)
	if n == 0 {
		return nil, false
	}
	return SubscribePath(p[:n-1]), true
}

type Index[T any] interface {
	Index(path ...uint32) (T, error)
}

type IndexReader interface {
	io.Reader
	io.ByteReader

	Index[IndexReader]
}

type ReaderFromIndex interface {
	ReadFromIndex(IndexReader) error
}

type IndexWriter interface {
	io.Writer
	io.ByteWriter

	Index[IndexWriter]
}

type WriterToIndex interface {
	WriteToIndex(IndexWriter) error
}

type Client interface {
	Invoke(ctx context.Context, instance string, name string, f func(IndexWriter, IndexReader, <-chan error) error, subs ...SubscribePath) error
	Serve(instance string, name string, f func(context.Context, IndexWriter, IndexReader, <-chan error) error, subs ...SubscribePath) (func() error, error)
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
