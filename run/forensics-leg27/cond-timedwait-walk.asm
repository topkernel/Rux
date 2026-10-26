
/tmp/ldmusl-u.so：     文件格式 elf64-x86-64


Disassembly of section .text:

00000000001182a0 <pthread_cond_timedwait+0xf0>:
  1182a0:	8b 07                	mov    (%rdi),%eax
  1182a2:	83 f8 02             	cmp    $0x2,%eax
  1182a5:	75 d9                	jne    118280 <pthread_cond_timedwait+0xd0>
  1182a7:	b8 ca 00 00 00       	mov    $0xca,%eax
  1182ac:	be 80 00 00 00       	mov    $0x80,%esi
  1182b1:	ba 02 00 00 00       	mov    $0x2,%edx
  1182b6:	45 31 d2             	xor    %r10d,%r10d
  1182b9:	0f 05                	syscall
  1182bb:	48 83 f8 da          	cmp    $0xffffffffffffffda,%rax
  1182bf:	75 df                	jne    1182a0 <pthread_cond_timedwait+0xf0>
  1182c1:	b8 ca 00 00 00       	mov    $0xca,%eax
  1182c6:	ba 02 00 00 00       	mov    $0x2,%edx
  1182cb:	31 f6                	xor    %esi,%esi
  1182cd:	45 31 d2             	xor    %r10d,%r10d
  1182d0:	0f 05                	syscall
  1182d2:	eb cc                	jmp    1182a0 <pthread_cond_timedwait+0xf0>
  1182d4:	48 8d 5d c4          	lea    -0x3c(%rbp),%rbx
  1182d8:	c7 45 c4 02 00 00 00 	movl   $0x2,-0x3c(%rbp)
  1182df:	c7 45 c0 00 00 00 00 	movl   $0x0,-0x40(%rbp)
  1182e6:	49 8b 46 08          	mov    0x8(%r14),%rax
  1182ea:	48 89 45 b8          	mov    %rax,-0x48(%rbp)
  1182ee:	48 8d 4d b0          	lea    -0x50(%rbp),%rcx
  1182f2:	49 89 4e 08          	mov    %rcx,0x8(%r14)
  1182f6:	49 8d 56 28          	lea    0x28(%r14),%rdx
  1182fa:	49 83 7e 28 00       	cmpq   $0x0,0x28(%r14)
  1182ff:	48 0f 45 d0          	cmovne %rax,%rdx
  118303:	48 89 0a             	mov    %rcx,(%rdx)
  118306:	45 87 46 20          	xchg   %r8d,0x20(%r14)
  11830a:	41 be 02 00 00 00    	mov    $0x2,%r14d
  118310:	41 83 f8 02          	cmp    $0x2,%r8d
  118314:	75 28                	jne    11833e <pthread_cond_timedwait+0x18e>
  118316:	b8 ca 00 00 00       	mov    $0xca,%eax
  11831b:	be 81 00 00 00       	mov    $0x81,%esi
  118320:	ba 01 00 00 00       	mov    $0x1,%edx
  118325:	0f 05                	syscall
  118327:	48 83 f8 da          	cmp    $0xffffffffffffffda,%rax
  11832b:	75 11                	jne    11833e <pthread_cond_timedwait+0x18e>
  11832d:	b8 ca 00 00 00       	mov    $0xca,%eax
  118332:	be 01 00 00 00       	mov    $0x1,%esi
  118337:	ba 01 00 00 00       	mov    $0x1,%edx
  11833c:	0f 05                	syscall
  11833e:	4c 89 e1             	mov    %r12,%rcx
  118341:	45 31 e4             	xor    %r12d,%r12d
  118344:	48 89 4d 98          	mov    %rcx,-0x68(%rbp)
  118348:	48 85 c9             	test   %rcx,%rcx
  11834b:	41 0f 94 c4          	sete   %r12b
  11834f:	4c 89 4d a0          	mov    %r9,-0x60(%rbp)
  118353:	4c 89 cf             	mov    %r9,%rdi
  118356:	e8 05 39 00 00       	call   11bc60 <pthread_mutex_unlock>
  11835b:	0f 1f 44 00 00       	nopl   0x0(%rax,%rax,1)
  118360:	48 89 df             	mov    %rbx,%rdi
  118363:	44 89 f6             	mov    %r14d,%esi
  118366:	44 89 ea             	mov    %r13d,%edx
  118369:	4c 89 f9             	mov    %r15,%rcx
  11836c:	45 89 e0             	mov    %r12d,%r8d
  11836f:	e8 cc e9 ff ff       	call   116d40 <tcsetwinsize+0x240>
  118374:	8b 0b                	mov    (%rbx),%ecx
  118376:	44 39 f1             	cmp    %r14d,%ecx
  118379:	75 07                	jne    118382 <pthread_cond_timedwait+0x1d2>
  11837b:	89 c1                	mov    %eax,%ecx
  11837d:	83 e1 fb             	and    $0xfffffffb,%ecx
  118380:	74 de                	je     118360 <pthread_cond_timedwait+0x1b0>
  118382:	45 31 e4             	xor    %r12d,%r12d
  118385:	83 f8 04             	cmp    $0x4,%eax
  118388:	44 0f 45 e0          	cmovne %eax,%r12d
  11838c:	48 83 7d 98 00       	cmpq   $0x0,-0x68(%rbp)
  118391:	4c 8b 45 a0          	mov    -0x60(%rbp),%r8
  118395:	74 6a                	je     118401 <pthread_cond_timedwait+0x251>
  118397:	41 83 fc 7d          	cmp    $0x7d,%r12d
  11839b:	75 16                	jne    1183b3 <pthread_cond_timedwait+0x203>
  11839d:	48 8b 45 a8          	mov    -0x58(%rbp),%rax
  1183a1:	8b 40 08             	mov    0x8(%rax),%eax
  1183a4:	31 c9                	xor    %ecx,%ecx
  1183a6:	44 39 f0             	cmp    %r14d,%eax
  1183a9:	41 bc 7d 00 00 00    	mov    $0x7d,%r12d
  1183af:	44 0f 45 e1          	cmovne %ecx,%r12d
  1183b3:	b8 ff ff ff ff       	mov    $0xffffffff,%eax
  1183b8:	48 8b 7d a8          	mov    -0x58(%rbp),%rdi
  1183bc:	f0 0f c1 47 0c       	lock xadd %eax,0xc(%rdi)
  1183c1:	3d 01 00 00 80       	cmp    $0x80000001,%eax
  1183c6:	0f 85 e9 02 00 00    	jne    1186b5 <pthread_cond_timedwait+0x505>
  1183cc:	48 83 c7 0c          	add    $0xc,%rdi
  1183d0:	b8 ca 00 00 00       	mov    $0xca,%eax
  1183d5:	be 01 00 00 00       	mov    $0x1,%esi
  1183da:	ba 01 00 00 00       	mov    $0x1,%edx
  1183df:	0f 05                	syscall
  1183e1:	48 83 f8 da          	cmp    $0xffffffffffffffda,%rax
  1183e5:	0f 85 ca 02 00 00    	jne    1186b5 <pthread_cond_timedwait+0x505>
  1183eb:	b8 ca 00 00 00       	mov    $0xca,%eax
  1183f0:	be 01 00 00 00       	mov    $0x1,%esi
  1183f5:	ba 01 00 00 00       	mov    $0x1,%edx
  1183fa:	0f 05                	syscall
  1183fc:	e9 b4 02 00 00       	jmp    1186b5 <pthread_cond_timedwait+0x505>
  118401:	b9 02 00 00 00       	mov    $0x2,%ecx
  118406:	31 c0                	xor    %eax,%eax
  118408:	f0 0f b1 4d c0       	lock cmpxchg %ecx,-0x40(%rbp)
  11840d:	85 c0                	test   %eax,%eax
  11840f:	0f 84 15 01 00 00    	je     11852a <pthread_cond_timedwait+0x37a>
  118415:	b9 01 00 00 00       	mov    $0x1,%ecx
  11841a:	31 c0                	xor    %eax,%eax
  11841c:	f0 0f b1 4d c4       	lock cmpxchg %ecx,-0x3c(%rbp)
  118421:	85 c0                	test   %eax,%eax
  118423:	74 70                	je     118495 <pthread_cond_timedwait+0x2e5>
  118425:	48 8d 7d c4          	lea    -0x3c(%rbp),%rdi
  118429:	bb 02 00 00 00       	mov    $0x2,%ebx
  11842e:	b8 01 00 00 00       	mov    $0x1,%eax
  118433:	f0 0f b1 5d c4       	lock cmpxchg %ebx,-0x3c(%rbp)
  118438:	b9 63 00 00 00       	mov    $0x63,%ecx
  11843d:	eb 11                	jmp    118450 <pthread_cond_timedwait+0x2a0>
  11843f:	90                   	nop
  118440:	31 c0                	xor    %eax,%eax
  118442:	f0 0f b1 5d c4       	lock cmpxchg %ebx,-0x3c(%rbp)
  118447:	b9 63 00 00 00       	mov    $0x63,%ecx
  11844c:	85 c0                	test   %eax,%eax
  11844e:	74 45                	je     118495 <pthread_cond_timedwait+0x2e5>
  118450:	8b 45 c4             	mov    -0x3c(%rbp),%eax
  118453:	83 f8 02             	cmp    $0x2,%eax
  118456:	75 e8                	jne    118440 <pthread_cond_timedwait+0x290>
  118458:	f3 90                	pause
  11845a:	83 e9 01             	sub    $0x1,%ecx
  11845d:	73 f1                	jae    118450 <pthread_cond_timedwait+0x2a0>
  11845f:	90                   	nop
  118460:	8b 45 c4             	mov    -0x3c(%rbp),%eax
  118463:	83 f8 02             	cmp    $0x2,%eax
  118466:	75 d8                	jne    118440 <pthread_cond_timedwait+0x290>
  118468:	b8 ca 00 00 00       	mov    $0xca,%eax
  11846d:	be 80 00 00 00       	mov    $0x80,%esi
  118472:	ba 02 00 00 00       	mov    $0x2,%edx
  118477:	45 31 d2             	xor    %r10d,%r10d
  11847a:	0f 05                	syscall
  11847c:	48 83 f8 da          	cmp    $0xffffffffffffffda,%rax
  118480:	75 de                	jne    118460 <pthread_cond_timedwait+0x2b0>
  118482:	b8 ca 00 00 00       	mov    $0xca,%eax
  118487:	ba 02 00 00 00       	mov    $0x2,%edx
  11848c:	31 f6                	xor    %esi,%esi
  11848e:	45 31 d2             	xor    %r10d,%r10d
  118491:	0f 05                	syscall
  118493:	eb cb                	jmp    118460 <pthread_cond_timedwait+0x2b0>
  118495:	4c 89 c7             	mov    %r8,%rdi
  118498:	e8 43 32 00 00       	call   11b6e0 <pthread_mutex_lock>
  11849d:	48 8b 55 a0          	mov    -0x60(%rbp),%rdx
  1184a1:	85 c0                	test   %eax,%eax
  1184a3:	44 0f 45 e0          	cmovne %eax,%r12d
  1184a7:	48 83 7d b8 00       	cmpq   $0x0,-0x48(%rbp)
  1184ac:	75 09                	jne    1184b7 <pthread_cond_timedwait+0x307>
  1184ae:	f6 02 08             	testb  $0x8,(%rdx)
  1184b1:	75 04                	jne    1184b7 <pthread_cond_timedwait+0x307>
  1184b3:	f0 ff 42 08          	lock incl 0x8(%rdx)
  1184b7:	48 8b 4d b0          	mov    -0x50(%rbp),%rcx
  1184bb:	48 85 c9             	test   %rcx,%rcx
  1184be:	0f 84 0d 01 00 00    	je     1185d1 <pthread_cond_timedwait+0x421>
  1184c4:	4c 8d 42 04          	lea    0x4(%rdx),%r8
  1184c8:	8b 42 04             	mov    0x4(%rdx),%eax
  1184cb:	85 c0                	test   %eax,%eax
  1184cd:	7e 11                	jle    1184e0 <pthread_cond_timedwait+0x330>
  1184cf:	89 c1                	mov    %eax,%ecx
  1184d1:	81 c9 00 00 00 80    	or     $0x80000000,%ecx
  1184d7:	f0 41 0f b1 08       	lock cmpxchg %ecx,(%r8)
  1184dc:	48 8b 4d b0          	mov    -0x50(%rbp),%rcx
  1184e0:	48 8d 79 14          	lea    0x14(%rcx),%rdi
  1184e4:	0f b6 02             	movzbl (%rdx),%eax
  1184e7:	31 d2                	xor    %edx,%edx
  1184e9:	89 51 14             	mov    %edx,0x14(%rcx)
  1184ec:	f0 83 0c 24 00       	lock orl $0x0,(%rsp)
  1184f1:	a8 88                	test   $0x88,%al
  1184f3:	0f 84 e3 00 00 00    	je     1185dc <pthread_cond_timedwait+0x42c>
  1184f9:	b8 ca 00 00 00       	mov    $0xca,%eax
  1184fe:	be 81 00 00 00       	mov    $0x81,%esi
  118503:	ba 01 00 00 00       	mov    $0x1,%edx
  118508:	0f 05                	syscall
  11850a:	48 83 f8 da          	cmp    $0xffffffffffffffda,%rax
  11850e:	0f 85 f6 00 00 00    	jne    11860a <pthread_cond_timedwait+0x45a>
  118514:	b8 ca 00 00 00       	mov    $0xca,%eax
  118519:	be 01 00 00 00       	mov    $0x1,%esi
  11851e:	ba 01 00 00 00       	mov    $0x1,%edx
  118523:	0f 05                	syscall
  118525:	e9 e0 00 00 00       	jmp    11860a <pthread_cond_timedwait+0x45a>
  11852a:	4c 8b 4d a8          	mov    -0x58(%rbp),%r9
  11852e:	49 8d 79 20          	lea    0x20(%r9),%rdi
  118532:	b9 01 00 00 00       	mov    $0x1,%ecx
  118537:	31 c0                	xor    %eax,%eax
  118539:	f0 41 0f b1 49 20    	lock cmpxchg %ecx,0x20(%r9)
  11853f:	85 c0                	test   %eax,%eax
  118541:	74 71                	je     1185b4 <pthread_cond_timedwait+0x404>
  118543:	bb 02 00 00 00       	mov    $0x2,%ebx
  118548:	b8 01 00 00 00       	mov    $0x1,%eax
  11854d:	f0 0f b1 1f          	lock cmpxchg %ebx,(%rdi)
  118551:	b9 63 00 00 00       	mov    $0x63,%ecx
  118556:	eb 17                	jmp    11856f <pthread_cond_timedwait+0x3bf>
  118558:	0f 1f 84 00 00 00 00 	nopl   0x0(%rax,%rax,1)
  11855f:	00 
  118560:	31 c0                	xor    %eax,%eax
  118562:	f0 0f b1 1f          	lock cmpxchg %ebx,(%rdi)
  118566:	b9 63 00 00 00       	mov    $0x63,%ecx
  11856b:	85 c0                	test   %eax,%eax
  11856d:	74 45                	je     1185b4 <pthread_cond_timedwait+0x404>
  11856f:	8b 07                	mov    (%rdi),%eax
  118571:	83 f8 02             	cmp    $0x2,%eax
  118574:	75 ea                	jne    118560 <pthread_cond_timedwait+0x3b0>
  118576:	f3 90                	pause
  118578:	83 e9 01             	sub    $0x1,%ecx
  11857b:	73 f2                	jae    11856f <pthread_cond_timedwait+0x3bf>
  11857d:	0f 1f 00             	nopl   (%rax)
  118580:	8b 07                	mov    (%rdi),%eax
  118582:	83 f8 02             	cmp    $0x2,%eax
  118585:	75 d9                	jne    118560 <pthread_cond_timedwait+0x3b0>
  118587:	b8 ca 00 00 00       	mov    $0xca,%eax
  11858c:	be 80 00 00 00       	mov    $0x80,%esi
  118591:	ba 02 00 00 00       	mov    $0x2,%edx
  118596:	45 31 d2             	xor    %r10d,%r10d
  118599:	0f 05                	syscall
  11859b:	48 83 f8 da          	cmp    $0xffffffffffffffda,%rax
  11859f:	75 df                	jne    118580 <pthread_cond_timedwait+0x3d0>
  1185a1:	b8 ca 00 00 00       	mov    $0xca,%eax
  1185a6:	ba 02 00 00 00       	mov    $0x2,%edx
  1185ab:	31 f6                	xor    %esi,%esi
  1185ad:	45 31 d2             	xor    %r10d,%r10d
  1185b0:	0f 05                	syscall
  1185b2:	eb cc                	jmp    118580 <pthread_cond_timedwait+0x3d0>
  1185b4:	48 8d 45 b0          	lea    -0x50(%rbp),%rax
  1185b8:	49 39 41 08          	cmp    %rax,0x8(%r9)
  1185bc:	74 5b                	je     118619 <pthread_cond_timedwait+0x469>
  1185be:	48 8b 4d b0          	mov    -0x50(%rbp),%rcx
  1185c2:	48 85 c9             	test   %rcx,%rcx
  1185c5:	74 5a                	je     118621 <pthread_cond_timedwait+0x471>
  1185c7:	48 8b 55 b8          	mov    -0x48(%rbp),%rdx
  1185cb:	48 89 51 08          	mov    %rdx,0x8(%rcx)
  1185cf:	eb 50                	jmp    118621 <pthread_cond_timedwait+0x471>
  1185d1:	f6 02 08             	testb  $0x8,(%rdx)
  1185d4:	75 34                	jne    11860a <pthread_cond_timedwait+0x45a>
  1185d6:	f0 ff 4a 08          	lock decl 0x8(%rdx)
  1185da:	eb 2e                	jmp    11860a <pthread_cond_timedwait+0x45a>
  1185dc:	b8 ca 00 00 00       	mov    $0xca,%eax
  1185e1:	be 83 00 00 00       	mov    $0x83,%esi
  1185e6:	41 ba 01 00 00 00    	mov    $0x1,%r10d
  1185ec:	31 d2                	xor    %edx,%edx
  1185ee:	0f 05                	syscall
  1185f0:	48 83 f8 da          	cmp    $0xffffffffffffffda,%rax
  1185f4:	75 14                	jne    11860a <pthread_cond_timedwait+0x45a>
  1185f6:	b8 ca 00 00 00       	mov    $0xca,%eax
  1185fb:	be 03 00 00 00       	mov    $0x3,%esi
  118600:	41 ba 01 00 00 00    	mov    $0x1,%r10d
  118606:	31 d2                	xor    %edx,%edx
  118608:	0f 05                	syscall
  11860a:	31 c0                	xor    %eax,%eax
  11860c:	41 83 fc 7d          	cmp    $0x7d,%r12d
  118610:	41 0f 45 c4          	cmovne %r12d,%eax
  118614:	e9 b5 00 00 00       	jmp    1186ce <pthread_cond_timedwait+0x51e>
  118619:	48 8b 4d b8          	mov    -0x48(%rbp),%rcx
  11861d:	49 89 49 08          	mov    %rcx,0x8(%r9)
  118621:	49 39 41 28          	cmp    %rax,0x28(%r9)
  118625:	74 12                	je     118639 <pthread_cond_timedwait+0x489>
  118627:	48 8b 45 b8          	mov    -0x48(%rbp),%rax
  11862b:	48 85 c0             	test   %rax,%rax
  11862e:	74 11                	je     118641 <pthread_cond_timedwait+0x491>
  118630:	48 8b 4d b0          	mov    -0x50(%rbp),%rcx
  118634:	48 89 08             	mov    %rcx,(%rax)
  118637:	eb 08                	jmp    118641 <pthread_cond_timedwait+0x491>
  118639:	48 8b 45 b0          	mov    -0x50(%rbp),%rax
  11863d:	49 89 41 28          	mov    %rax,0x28(%r9)
  118641:	31 c0                	xor    %eax,%eax
  118643:	87 07                	xchg   %eax,(%rdi)
  118645:	83 f8 02             	cmp    $0x2,%eax
  118648:	75 28                	jne    118672 <pthread_cond_timedwait+0x4c2>
  11864a:	b8 ca 00 00 00       	mov    $0xca,%eax
  11864f:	be 81 00 00 00       	mov    $0x81,%esi
  118654:	ba 01 00 00 00       	mov    $0x1,%edx
  118659:	0f 05                	syscall
  11865b:	48 83 f8 da          	cmp    $0xffffffffffffffda,%rax
  11865f:	75 11                	jne    118672 <pthread_cond_timedwait+0x4c2>
  118661:	b8 ca 00 00 00       	mov    $0xca,%eax
  118666:	be 01 00 00 00       	mov    $0x1,%esi
  11866b:	ba 01 00 00 00       	mov    $0x1,%edx
  118670:	0f 05                	syscall
  118672:	48 8b 45 c8          	mov    -0x38(%rbp),%rax
  118676:	48 85 c0             	test   %rax,%rax
  118679:	74 3a                	je     1186b5 <pthread_cond_timedwait+0x505>
  11867b:	b9 ff ff ff ff       	mov    $0xffffffff,%ecx
  118680:	f0 0f c1 08          	lock xadd %ecx,(%rax)
  118684:	83 f9 01             	cmp    $0x1,%ecx
  118687:	75 2c                	jne    1186b5 <pthread_cond_timedwait+0x505>
  118689:	48 8b 7d c8          	mov    -0x38(%rbp),%rdi
  11868d:	b8 ca 00 00 00       	mov    $0xca,%eax
  118692:	be 81 00 00 00       	mov    $0x81,%esi
  118697:	ba 01 00 00 00       	mov    $0x1,%edx
  11869c:	0f 05                	syscall
  11869e:	48 83 f8 da          	cmp    $0xffffffffffffffda,%rax
  1186a2:	75 11                	jne    1186b5 <pthread_cond_timedwait+0x505>
  1186a4:	b8 ca 00 00 00       	mov    $0xca,%eax
  1186a9:	be 01 00 00 00       	mov    $0x1,%esi
  1186ae:	ba 01 00 00 00       	mov    $0x1,%edx
  1186b3:	0f 05                	syscall
  1186b5:	4c 89 c7             	mov    %r8,%rdi
  1186b8:	e8 23 30 00 00       	call   11b6e0 <pthread_mutex_lock>
  1186bd:	85 c0                	test   %eax,%eax
  1186bf:	44 0f 45 e0          	cmovne %eax,%r12d
  1186c3:	b8 7d 00 00 00       	mov    $0x7d,%eax
  1186c8:	41 83 fc 7d          	cmp    $0x7d,%r12d
  1186cc:	75 03                	jne    1186d1 <pthread_cond_timedwait+0x521>
  1186ce:	41 89 c4             	mov    %eax,%r12d
  1186d1:	64 48 8b 04 25 28 00 	mov    %fs:0x28,%rax
  1186d8:	00 00 
  1186da:	48 3b 45 d0          	cmp    -0x30(%rbp),%rax
  1186de:	75 12                	jne    1186f2 <pthread_cond_timedwait+0x542>
  1186e0:	44 89 e0             	mov    %r12d,%eax
  1186e3:	48 83 c4 48          	add    $0x48,%rsp
  1186e7:	5b                   	pop    %rbx
  1186e8:	41 5c                	pop    %r12
  1186ea:	41 5d                	pop    %r13
  1186ec:	41 5e                	pop    %r14
  1186ee:	41 5f                	pop    %r15
  1186f0:	5d                   	pop    %rbp
  1186f1:	c3                   	ret
  1186f2:	e8 c9 20 f7 ff       	call   8a7c0 <__stack_chk_fail>
  1186f7:	cc                   	int3
  1186f8:	cc                   	int3
  1186f9:	cc                   	int3
  1186fa:	cc                   	int3
  1186fb:	cc                   	int3
  1186fc:	cc                   	int3
  1186fd:	cc                   	int3
  1186fe:	cc                   	int3
  1186ff:	cc                   	int3
  118700:	55                   	push   %rbp
  118701:	48 89 e5             	mov    %rsp,%rbp
  118704:	53                   	push   %rbx
  118705:	48 83 ec 18          	sub    $0x18,%rsp
  118709:	89 f3                	mov    %esi,%ebx
  11870b:	49 89 f9             	mov    %rdi,%r9
  11870e:	64 48 8b 04 25 28 00 	mov    %fs:0x28,%rax
  118715:	00 00 
  118717:	48 89 45 f0          	mov    %rax,-0x10(%rbp)
  11871b:	c7 45 ec 00 00 00 00 	movl   $0x0,-0x14(%rbp)
  118722:	b9 01 00 00 00       	mov    $0x1,%ecx
  118727:	31 c0                	xor    %eax,%eax
  118729:	f0 0f b1 4f 20       	lock cmpxchg %ecx,0x20(%rdi)
  11872e:	48 83 c7 20          	add    $0x20,%rdi
  118732:	85 c0                	test   %eax,%eax
  118734:	74 6e                	je     1187a4 <pthread_cond_timedwait+0x5f4>
  118736:	41 b8 02 00 00 00    	mov    $0x2,%r8d
  11873c:	b8 01 00 00 00       	mov    $0x1,%eax
  118741:	f0 44 0f b1 07       	lock cmpxchg %r8d,(%rdi)
  118746:	b9 63 00 00 00       	mov    $0x63,%ecx
  11874b:	eb 13                	jmp    118760 <pthread_cond_timedwait+0x5b0>
  11874d:	0f 1f 00             	nopl   (%rax)
  118750:	31 c0                	xor    %eax,%eax
  118752:	f0 44 0f b1 07       	lock cmpxchg %r8d,(%rdi)
  118757:	b9 63 00 00 00       	mov    $0x63,%ecx
  11875c:	85 c0                	test   %eax,%eax
  11875e:	74 44                	je     1187a4 <pthread_cond_timedwait+0x5f4>
  118760:	8b 07                	mov    (%rdi),%eax
  118762:	83 f8 02             	cmp    $0x2,%eax
  118765:	75 e9                	jne    118750 <pthread_cond_timedwait+0x5a0>
  118767:	f3 90                	pause
  118769:	83 e9 01             	sub    $0x1,%ecx
  11876c:	73 f2                	jae    118760 <pthread_cond_timedwait+0x5b0>
  11876e:	66 90                	xchg   %ax,%ax
  118770:	8b 07                	mov    (%rdi),%eax
  118772:	83 f8 02             	cmp    $0x2,%eax
  118775:	75 d9                	jne    118750 <pthread_cond_timedwait+0x5a0>
  118777:	b8 ca 00 00 00       	mov    $0xca,%eax
  11877c:	be 80 00 00 00       	mov    $0x80,%esi
  118781:	ba 02 00 00 00       	mov    $0x2,%edx
  118786:	45 31 d2             	xor    %r10d,%r10d
  118789:	0f 05                	syscall
  11878b:	48 83 f8 da          	cmp    $0xffffffffffffffda,%rax
  11878f:	75 df                	jne    118770 <pthread_cond_timedwait+0x5c0>
  118791:	b8 ca 00 00 00       	mov    $0xca,%eax
  118796:	ba 02 00 00 00       	mov    $0x2,%edx
  11879b:	31 f6                	xor    %esi,%esi
  11879d:	45 31 d2             	xor    %r10d,%r10d
  1187a0:	0f 05                	syscall
  1187a2:	eb cc                	jmp    118770 <pthread_cond_timedwait+0x5c0>
  1187a4:	49 8b 49 28          	mov    0x28(%r9),%rcx
  1187a8:	48 85 c9             	test   %rcx,%rcx
  1187ab:	0f 95 c0             	setne  %al
  1187ae:	45 31 c0             	xor    %r8d,%r8d
  1187b1:	85 db                	test   %ebx,%ebx
  1187b3:	74 56                	je     11880b <pthread_cond_timedwait+0x65b>
  1187b5:	48 85 c9             	test   %rcx,%rcx
  1187b8:	74 51                	je     11880b <pthread_cond_timedwait+0x65b>
  1187ba:	45 31 c0             	xor    %r8d,%r8d
  1187bd:	ba 01 00 00 00       	mov    $0x1,%edx
  1187c2:	48 8d 75 ec          	lea    -0x14(%rbp),%rsi
  1187c6:	66 2e 0f 1f 84 00 00 	cs nopw 0x0(%rax,%rax,1)
  1187cd:	00 00 00 
  1187d0:	31 c0                	xor    %eax,%eax
  1187d2:	f0 0f b1 51 10       	lock cmpxchg %edx,0x10(%rcx)
  1187d7:	85 c0                	test   %eax,%eax
  1187d9:	74 15                	je     1187f0 <pthread_cond_timedwait+0x640>
  1187db:	ff 45 ec             	incl   -0x14(%rbp)
  1187de:	48 89 71 18          	mov    %rsi,0x18(%rcx)
  1187e2:	eb 15                	jmp    1187f9 <pthread_cond_timedwait+0x649>
  1187e4:	66 66 66 2e 0f 1f 84 	data16 data16 cs nopw 0x0(%rax,%rax,1)
  1187eb:	00 00 00 00 00 
  1187f0:	ff cb                	dec    %ebx
  1187f2:	4d 85 c0             	test   %r8,%r8
  1187f5:	4c 0f 44 c1          	cmove  %rcx,%r8
  1187f9:	48 8b 09             	mov    (%rcx),%rcx
  1187fc:	48 85 c9             	test   %rcx,%rcx
  1187ff:	0f 95 c0             	setne  %al
  118802:	85 db                	test   %ebx,%ebx
  118804:	74 05                	je     11880b <pthread_cond_timedwait+0x65b>
  118806:	48 85 c9             	test   %rcx,%rcx
  118809:	75 c5                	jne    1187d0 <pthread_cond_timedwait+0x620>
  11880b:	84 c0                	test   %al,%al
  11880d:	74 16                	je     118825 <pthread_cond_timedwait+0x675>
  11880f:	48 8d 41 08          	lea    0x8(%rcx),%rax
  118813:	48 8b 51 08          	mov    0x8(%rcx),%rdx
  118817:	48 85 d2             	test   %rdx,%rdx
  11881a:	74 0d                	je     118829 <pthread_cond_timedwait+0x679>
  11881c:	48 c7 02 00 00 00 00 	movq   $0x0,(%rdx)
  118823:	eb 04                	jmp    118829 <pthread_cond_timedwait+0x679>
  118825:	49 8d 41 08          	lea    0x8(%r9),%rax
  118829:	48 c7 00 00 00 00 00 	movq   $0x0,(%rax)
  118830:	49 89 49 28          	mov    %rcx,0x28(%r9)
  118834:	31 c0                	xor    %eax,%eax
  118836:	41 87 41 20          	xchg   %eax,0x20(%r9)
  11883a:	83 f8 02             	cmp    $0x2,%eax
  11883d:	75 28                	jne    118867 <pthread_cond_timedwait+0x6b7>
  11883f:	b8 ca 00 00 00       	mov    $0xca,%eax
  118844:	be 81 00 00 00       	mov    $0x81,%esi
  118849:	ba 01 00 00 00       	mov    $0x1,%edx
  11884e:	0f 05                	syscall
  118850:	48 83 f8 da          	cmp    $0xffffffffffffffda,%rax
  118854:	75 11                	jne    118867 <pthread_cond_timedwait+0x6b7>
  118856:	b8 ca 00 00 00       	mov    $0xca,%eax
  11885b:	be 01 00 00 00       	mov    $0x1,%esi
  118860:	ba 01 00 00 00       	mov    $0x1,%edx
  118865:	0f 05                	syscall
  118867:	8b 5d ec             	mov    -0x14(%rbp),%ebx
  11886a:	85 db                	test   %ebx,%ebx
  11886c:	74 6c                	je     1188da <pthread_cond_timedwait+0x72a>
  11886e:	48 8d 7d ec          	lea    -0x14(%rbp),%rdi
  118872:	eb 13                	jmp    118887 <pthread_cond_timedwait+0x6d7>
  118874:	66 66 66 2e 0f 1f 84 	data16 data16 cs nopw 0x0(%rax,%rax,1)
  11887b:	00 00 00 00 00 
  118880:	8b 5d ec             	mov    -0x14(%rbp),%ebx
  118883:	85 db                	test   %ebx,%ebx
  118885:	74 53                	je     1188da <pthread_cond_timedwait+0x72a>
  118887:	b8 9c ff ff ff       	mov    $0xffffff9c,%eax
  11888c:	0f 1f 40 00          	nopl   0x0(%rax)
  118890:	8b 4d ec             	mov    -0x14(%rbp),%ecx
  118893:	39 d9                	cmp    %ebx,%ecx
  118895:	75 e9                	jne    118880 <pthread_cond_timedwait+0x6d0>
  118897:	f3 90                	pause
  118899:	ff c0                	inc    %eax
  11889b:	75 f3                	jne    118890 <pthread_cond_timedwait+0x6e0>
  11889d:	8b 45 ec             	mov    -0x14(%rbp),%eax
  1188a0:	39 d8                	cmp    %ebx,%eax
  1188a2:	75 dc                	jne    118880 <pthread_cond_timedwait+0x6d0>
  1188a4:	48 63 d3             	movslq %ebx,%rdx
  1188a7:	eb 0e                	jmp    1188b7 <pthread_cond_timedwait+0x707>
  1188a9:	0f 1f 80 00 00 00 00 	nopl   0x0(%rax)
  1188b0:	8b 45 ec             	mov    -0x14(%rbp),%eax
  1188b3:	39 d8                	cmp    %ebx,%eax
  1188b5:	75 c9                	jne    118880 <pthread_cond_timedwait+0x6d0>
  1188b7:	b8 ca 00 00 00       	mov    $0xca,%eax
  1188bc:	be 80 00 00 00       	mov    $0x80,%esi
  1188c1:	45 31 d2             	xor    %r10d,%r10d
  1188c4:	0f 05                	syscall
  1188c6:	48 83 f8 da          	cmp    $0xffffffffffffffda,%rax
  1188ca:	75 e4                	jne    1188b0 <pthread_cond_timedwait+0x700>
  1188cc:	b8 ca 00 00 00       	mov    $0xca,%eax
  1188d1:	31 f6                	xor    %esi,%esi
  1188d3:	45 31 d2             	xor    %r10d,%r10d
  1188d6:	0f 05                	syscall
  1188d8:	eb d6                	jmp    1188b0 <pthread_cond_timedwait+0x700>
  1188da:	4d 85 c0             	test   %r8,%r8
  1188dd:	74 3d                	je     11891c <pthread_cond_timedwait+0x76c>
  1188df:	31 c0                	xor    %eax,%eax
  1188e1:	41 87 40 14          	xchg   %eax,0x14(%r8)
  1188e5:	83 f8 02             	cmp    $0x2,%eax
  1188e8:	75 32                	jne    11891c <pthread_cond_timedwait+0x76c>
  1188ea:	49 83 c0 14          	add    $0x14,%r8
  1188ee:	b8 ca 00 00 00       	mov    $0xca,%eax
  1188f3:	be 81 00 00 00       	mov    $0x81,%esi
  1188f8:	ba 01 00 00 00       	mov    $0x1,%edx
  1188fd:	4c 89 c7             	mov    %r8,%rdi
