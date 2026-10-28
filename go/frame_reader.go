package wrpc

import (
	"context"
	"encoding/binary"
	"errors"
	"fmt"
	"io"
	"log/slog"
	"runtime"
	"strings"
	"sync"
	"sync/atomic"
)

const maxUnclaimedElements = 64

func indexPath(path ...uint32) string {
	p := make([]byte, len(path)*4)
	for i, v := range path {
		l := i * 4
		binary.LittleEndian.PutUint32(p[l:l+4], v)
	}
	return string(p)
}

func concretePath(path SubscribePath) (string, bool) {
	p := make([]byte, len(path)*4)
	for i, v := range path {
		if v == nil {
			return "", false
		}
		l := i * 4
		binary.LittleEndian.PutUint32(p[l:l+4], *v)
	}
	return string(p), true
}

func decodePath(p string) []uint32 {
	path := make([]uint32, len(p)/4)
	for i := range path {
		l := i * 4
		path[i] = binary.LittleEndian.Uint32([]byte(p[l : l+4]))
	}
	return path
}

type subscribePattern struct {
	declared bool
	wildcard *subscribePattern
	nested   map[uint32]*subscribePattern
}

func (p *subscribePattern) insert(path SubscribePath) {
	for _, i := range path {
		if i == nil {
			if p.wildcard == nil {
				p.wildcard = &subscribePattern{}
			}
			p = p.wildcard
			continue
		}
		if p.nested == nil {
			p.nested = make(map[uint32]*subscribePattern)
		}
		n, ok := p.nested[*i]
		if !ok {
			n = &subscribePattern{}
			p.nested[*i] = n
		}
		p = n
	}
	p.declared = true
}

func (p *subscribePattern) resolve(path []uint32) (*subscribePattern, []int) {
	var elements []int
	for k, i := range path {
		if p.wildcard != nil {
			elements = append(elements, k)
			p = p.wildcard
		} else if p = p.nested[i]; p == nil {
			return nil, nil
		}
	}
	return p, elements
}

type frameElement struct {
	readers    int
	parent     *frameElement
	paths      []string
	containers []string
	elements   map[string]struct{}
}

type frameSub struct {
	ch      chan []byte
	dropped chan struct{}
	taken   bool
}

func newFrameSub() *frameSub {
	return &frameSub{
		ch:      make(chan []byte, 16),
		dropped: make(chan struct{}),
	}
}

func (s *frameSub) drop() {
	select {
	case <-s.dropped:
	default:
		close(s.dropped)
	}
}

func (s *frameSub) isDropped() bool {
	select {
	case <-s.dropped:
		return true
	default:
		return false
	}
}

type frameSubscriptions struct {
	ctx         context.Context
	mu          sync.Mutex
	cond        *sync.Cond
	patterns    subscribePattern
	nest        map[string]*frameSub
	closed      map[string]struct{}
	elements    map[string]*frameElement
	unclaimed   map[string]struct{}
	next        map[string]uint64
	done        bool
	drained     bool
	unprocessed bool
}

func errNotFound(path []uint32) error {
	return fmt.Errorf("`%v` subscription not found", path)
}

var errUnprocessed = errors.New("received data was not processed")

func (s *frameSubscriptions) owner(path []uint32, positions []int, k int) *frameElement {
	var el *frameElement
	for _, j := range positions {
		if j >= k {
			break
		}
		el = s.elements[indexPath(path[:j+1]...)]
	}
	return el
}

func (s *frameSubscriptions) element(path []uint32, positions []int, k int) (string, *frameElement) {
	e := indexPath(path[:k+1]...)
	el, ok := s.elements[e]
	if !ok {
		el = &frameElement{
			parent:   s.owner(path, positions, k),
			elements: make(map[string]struct{}),
		}
		if el.parent != nil {
			el.parent.elements[e] = struct{}{}
		}
		s.elements[e] = el
	}
	return e, el
}

func (s *frameSubscriptions) subscribe(path []uint32, positions []int) *frameSub {
	p := indexPath(path...)
	sub, ok := s.nest[p]
	if !ok {
		sub = newFrameSub()
		if s.done {
			close(sub.ch)
		}
		if s.drained {
			sub.drop()
		}
		s.nest[p] = sub
		if el := s.owner(path, positions, len(path)); el != nil {
			el.paths = append(el.paths, p)
		}
	}
	return sub
}

func (s *frameSubscriptions) route(path []uint32, p string) (*frameSub, bool, error) {
	if sub, ok := s.nest[p]; ok {
		return sub, false, nil
	}
	for {
		pattern, positions := s.patterns.resolve(path)
		if pattern == nil || !pattern.declared {
			return nil, false, nil
		}
		missing := 0
		for _, k := range positions {
			if _, ok := s.elements[indexPath(path[:k+1]...)]; ok {
				continue
			}
			if uint64(path[k]) < s.next[indexPath(path[:k]...)] {
				return nil, true, nil
			}
			missing++
		}
		if missing == 0 || !s.drained && len(s.unclaimed)+missing <= maxUnclaimedElements {
			for _, k := range positions {
				if _, ok := s.elements[indexPath(path[:k+1]...)]; !ok {
					e, _ := s.element(path, positions, k)
					s.unclaimed[e] = struct{}{}
				}
			}
			return s.subscribe(path, positions), false, nil
		}
		if s.drained {
			return nil, true, nil
		}
		if err := s.ctx.Err(); err != nil {
			return nil, false, err
		}
		s.cond.Wait()
	}
}

func (s *frameSubscriptions) claim(path []uint32) (*frameSub, []string, error) {
	pattern, positions := s.patterns.resolve(path)
	if pattern == nil {
		return nil, nil, errNotFound(path)
	}
	for _, k := range positions {
		if _, ok := s.elements[indexPath(path[:k+1]...)]; !ok && uint64(path[k]) < s.next[indexPath(path[:k]...)] {
			return nil, nil, errors.New("released subscription")
		}
	}
	if pattern.declared && pattern.wildcard == nil && len(pattern.nested) == 0 {
		if sub, ok := s.nest[indexPath(path...)]; ok && sub.taken {
			return nil, nil, errNotFound(path)
		}
	}
	elements := make([]string, len(positions))
	for j, k := range positions {
		c := indexPath(path[:k]...)
		next, ok := s.next[c]
		for p := range s.unclaimed {
			if len(p) != len(c)+4 || !strings.HasPrefix(p, c) {
				continue
			}
			if i := uint64(binary.LittleEndian.Uint32([]byte(p[len(c):]))); i >= next && i < uint64(path[k]) {
				s.remove(p)
			}
		}
		e, el := s.element(path, positions, k)
		el.readers++
		delete(s.unclaimed, e)
		if !ok {
			if owner := s.owner(path, positions, k); owner != nil {
				owner.containers = append(owner.containers, c)
			}
		}
		s.next[c] = max(next, uint64(path[k])+1)
		elements[j] = e
	}
	s.cond.Broadcast()
	if !pattern.declared {
		return nil, elements, nil
	}
	sub := s.subscribe(path, positions)
	if sub.taken {
		return nil, elements, nil
	}
	sub.taken = true
	return sub, elements, nil
}

func (s *frameSubscriptions) remove(e string) {
	el, ok := s.elements[e]
	if !ok {
		return
	}
	delete(s.elements, e)
	delete(s.unclaimed, e)
	if el.parent != nil {
		delete(el.parent.elements, e)
	}
	for _, p := range el.paths {
		if sub, ok := s.nest[p]; ok {
			if len(sub.ch) > 0 {
				s.unprocessed = true
			}
			sub.drop()
		}
		delete(s.nest, p)
		delete(s.closed, p)
	}
	for _, c := range el.containers {
		delete(s.next, c)
	}
	for n := range el.elements {
		s.remove(n)
	}
	s.cond.Broadcast()
}

type FrameStreamReader struct {
	ctx      context.Context
	sub      *frameSub
	readers  *atomic.Int64
	subs     *frameSubscriptions
	elements []string
	err      *atomic.Pointer[error]
	path     string
	buf      []byte
	gone     atomic.Bool
}

func newFrameStreamReader(r *FrameStreamReader) *FrameStreamReader {
	runtime.SetFinalizer(r, func(r *FrameStreamReader) {
		slog.DebugContext(r.ctx, "closing unused stream reader")
		r.drop()
	})
	return r
}

func NewFrameStreamReader(ctx context.Context, r ByteReadCloser, paths ...SubscribePath) *FrameStreamReader {
	var readers atomic.Int64
	readers.Add(1)

	subs := &frameSubscriptions{
		ctx:       ctx,
		nest:      make(map[string]*frameSub, len(paths)+1),
		closed:    make(map[string]struct{}),
		elements:  make(map[string]*frameElement),
		unclaimed: make(map[string]struct{}),
		next:      make(map[string]uint64),
	}
	subs.cond = sync.NewCond(&subs.mu)
	root := newFrameSub()
	root.taken = true
	subs.nest[""] = root
	for _, path := range paths {
		subs.patterns.insert(path)
		if p, ok := concretePath(path); ok {
			subs.nest[p] = newFrameSub()
		}
	}
	stop := context.AfterFunc(ctx, func() {
		subs.mu.Lock()
		subs.cond.Broadcast()
		subs.mu.Unlock()
	})

	var ingressErr atomic.Pointer[error]
	go func() {
		accept := func(path []uint32, p string, n uint32) (*frameSub, error) {
			subs.mu.Lock()
			defer subs.mu.Unlock()
			sub, released, err := subs.route(path, p)
			if err != nil {
				return nil, err
			}
			_, closed := subs.closed[p]
			if n == 0 {
				if sub == nil || closed {
					return nil, nil
				}
				slog.DebugContext(ctx, "received empty frame, closing path", "path", path)
				subs.closed[p] = struct{}{}
				close(sub.ch)
				return nil, nil
			}
			if released {
				return nil, fmt.Errorf("received data for a released element on path `%v`", path)
			}
			if sub == nil {
				return nil, fmt.Errorf("received a frame for unknown path `%v`", path)
			}
			if closed {
				return nil, fmt.Errorf("received a frame for closed path `%v`", path)
			}
			if sub.isDropped() {
				return nil, fmt.Errorf("received data for a closed reader on path `%v`", path)
			}
			return sub, nil
		}
		handle := func(path []uint32, n uint32) error {
			sub, err := accept(path, indexPath(path...), n)
			if err != nil || sub == nil {
				return err
			}
			for n > 0 {
				k := min(n, maxInitialCapacity)
				data := make([]byte, k)
				if _, err := io.ReadFull(r, data); err != nil {
					if err == io.EOF {
						err = io.ErrUnexpectedEOF
					}
					return fmt.Errorf("failed to read data: %w", err)
				}
				n -= k
				slog.DebugContext(ctx, "sending received frame data",
					"path", path,
					"len", k,
				)
				select {
				case <-ctx.Done():
					return ctx.Err()
				case <-sub.dropped:
					return fmt.Errorf("received data for a closed reader on path `%v`", path)
				case sub.ch <- data:
				}
				subs.mu.Lock()
				dropped := sub.isDropped() && len(sub.ch) > 0
				subs.mu.Unlock()
				if dropped {
					return fmt.Errorf("received data for a closed reader on path `%v`", path)
				}
			}
			return nil
		}
		err := func() error {
			for {
				if err := ctx.Err(); err != nil {
					return err
				}
				subs.mu.Lock()
				unprocessed := subs.drained && subs.unprocessed
				subs.mu.Unlock()
				if unprocessed {
					return errUnprocessed
				}
				slog.DebugContext(ctx, "reading frame")
				path, n, err := readFrameHeader(r)
				if err == io.EOF {
					return nil
				}
				if err != nil {
					return err
				}
				slog.DebugContext(ctx, "read frame header",
					"path", path,
					"len", n,
				)
				if err := handle(path, n); err != nil {
					return err
				}
			}
		}()
		if err != nil {
			ingressErr.Store(&err)
		}
		subs.mu.Lock()
		subs.done = true
		for p, sub := range subs.nest {
			if _, ok := subs.closed[p]; !ok {
				close(sub.ch)
			}
		}
		if err == nil {
			for !subs.drained && ctx.Err() == nil {
				subs.cond.Wait()
			}
			if subs.drained && subs.unprocessed {
				err = errUnprocessed
			}
		}
		subs.mu.Unlock()
		stop()
		if err != nil {
			slog.ErrorContext(ctx, "ingress failed", "err", err)
		} else {
			slog.DebugContext(ctx, "ingress successfully complete")
		}
		if err := r.Close(); err != nil {
			slog.DebugContext(ctx, "failed to close frame stream reader", "err", err)
		}
	}()
	return newFrameStreamReader(&FrameStreamReader{
		ctx:     ctx,
		sub:     root,
		readers: &readers,
		subs:    subs,
		err:     &ingressErr,
	})
}

func (r *FrameStreamReader) Read(p []byte) (int, error) {
	slog.DebugContext(r.ctx, "reading bytes",
		"path", r.path,
		"requested", len(p),
	)
	if len(r.buf) > 0 {
		n := copy(p, r.buf)
		slog.DebugContext(r.ctx, "copied bytes from buffer",
			"path", r.path,
			"requested", len(p),
			"buffered", len(r.buf),
			"copied", n,
		)
		r.buf = r.buf[n:]
		return n, nil
	}
	if r.sub == nil {
		return 0, errNotFound(decodePath(r.path))
	}
	slog.DebugContext(r.ctx, "waiting for next frame",
		"path", r.path,
	)
	select {
	case <-r.ctx.Done():
		return 0, r.ctx.Err()
	case buf, ok := <-r.sub.ch:
		if !ok {
			return 0, r.closedErr()
		}
		slog.DebugContext(r.ctx, "received data frame",
			"path", r.path,
			"buf", buf,
		)
		n := copy(p, buf)
		if n < len(buf) {
			r.buf = buf[n:]
		}
		return n, nil
	}
}

func (r *FrameStreamReader) ReadByte() (byte, error) {
	slog.DebugContext(r.ctx, "reading byte", "path", r.path)
	if len(r.buf) > 0 {
		b := r.buf[0]
		r.buf = r.buf[1:]
		slog.DebugContext(r.ctx, "copied byte from buffer",
			"path", r.path,
			"buffered", len(r.buf),
		)
		return b, nil
	}
	if r.sub == nil {
		return 0, errNotFound(decodePath(r.path))
	}
	slog.DebugContext(r.ctx, "waiting for next frame", "path", r.path)
	select {
	case <-r.ctx.Done():
		return 0, r.ctx.Err()
	case buf, ok := <-r.sub.ch:
		if !ok {
			return 0, r.closedErr()
		}
		slog.DebugContext(r.ctx, "received data frame", "path", r.path)
		if len(buf) > 1 {
			r.buf = buf[1:]
		}
		return buf[0], nil
	}
}

func (r *FrameStreamReader) Buffered() int {
	return len(r.buf)
}

func (r *FrameStreamReader) closedErr() error {
	r.subs.mu.Lock()
	_, closed := r.subs.closed[r.path]
	r.subs.mu.Unlock()
	if err := r.err.Load(); err != nil && !closed {
		return *err
	}
	return io.EOF
}

func (r *FrameStreamReader) Index(path ...uint32) (IndexReadCloser, error) {
	if len(path) == 0 {
		return nil, errors.New("path cannot be empty")
	}
	full := append(decodePath(r.path), path...)
	r.subs.mu.Lock()
	defer r.subs.mu.Unlock()
	sub, elements, err := r.subs.claim(full)
	if err != nil {
		return nil, err
	}
	readers := r.readers.Add(1)
	slog.DebugContext(r.ctx, "indexing reader",
		"readers", readers,
		"path", full,
	)
	return newFrameStreamReader(&FrameStreamReader{
		ctx:      r.ctx,
		sub:      sub,
		readers:  r.readers,
		subs:     r.subs,
		elements: elements,
		err:      r.err,
		path:     indexPath(full...),
	}), nil
}

func (r *FrameStreamReader) drop() {
	if r.gone.Swap(true) {
		return
	}
	readers := r.readers.Add(-1)
	slog.DebugContext(r.ctx, "dropped frame stream reader",
		"readers", readers,
		"path", r.path,
	)

	s := r.subs
	s.mu.Lock()
	defer s.mu.Unlock()

	if r.sub != nil {
		if len(r.buf) > 0 || len(r.sub.ch) > 0 {
			s.unprocessed = true
		}
		r.sub.drop()
	}
	for _, e := range r.elements {
		el, ok := s.elements[e]
		if !ok {
			break
		}
		el.readers--
		if el.readers == 0 {
			s.remove(e)
			break
		}
	}
	if readers == 0 {
		s.drained = true
		for _, sub := range s.nest {
			if len(sub.ch) > 0 {
				s.unprocessed = true
			}
			sub.drop()
		}
	}
	s.cond.Broadcast()
}

func (r *FrameStreamReader) Close() error {
	defer runtime.SetFinalizer(r, nil)
	r.drop()
	return nil
}
