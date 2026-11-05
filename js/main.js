/* Rux 主页交互:代码复制 / 截图灯箱 / 回到顶部 / 阅读进度 / 滚动显现 / 数字滚动 */
(function () {
  'use strict';

  var reduced = window.matchMedia && window.matchMedia('(prefers-reduced-motion: reduce)').matches;

  // 代码块复制按钮
  document.querySelectorAll('pre').forEach(function (pre) {
    var btn = document.createElement('button');
    btn.className = 'copy-btn';
    btn.textContent = '复制';
    btn.addEventListener('click', function () {
      var code = pre.querySelector('code');
      if (!code) return;
      navigator.clipboard.writeText(code.innerText).then(function () {
        btn.textContent = '已复制 ✓';
        setTimeout(function () { btn.textContent = '复制'; }, 1600);
      });
    });
    pre.appendChild(btn);
  });

  // 截图灯箱
  var lb = document.createElement('div');
  lb.id = 'lightbox';
  var img = document.createElement('img');
  lb.appendChild(img);
  document.body.appendChild(lb);
  document.querySelectorAll('.shot img, .gallery img').forEach(function (el) {
    el.addEventListener('click', function () {
      img.src = el.getAttribute('data-full') || el.src;
      lb.classList.add('on');
    });
  });
  lb.addEventListener('click', function () { lb.classList.remove('on'); });
  document.addEventListener('keydown', function (e) {
    if (e.key === 'Escape') lb.classList.remove('on');
  });

  // 回到顶部
  var top = document.createElement('button');
  top.id = 'toTop';
  top.textContent = '↑';
  top.title = '回到顶部';
  top.addEventListener('click', function () {
    window.scrollTo({ top: 0, behavior: reduced ? 'auto' : 'smooth' });
  });
  document.body.appendChild(top);
  window.addEventListener('scroll', function () {
    top.classList.toggle('on', window.scrollY > 600);
  }, { passive: true });

  // 阅读进度条(scroll-progress-timeline)
  var pbar = document.createElement('div');
  pbar.id = 'pbar';
  document.body.appendChild(pbar);
  var ticking = false;
  window.addEventListener('scroll', function () {
    if (ticking) return;
    ticking = true;
    requestAnimationFrame(function () {
      var h = document.documentElement.scrollHeight - window.innerHeight;
      pbar.style.width = (h > 0 ? (window.scrollY / h) * 100 : 0) + '%';
      ticking = false;
    });
  }, { passive: true });

  // 滚动显现 + 数字滚动(animation-on-scroll 模式,once=true)
  if (!('IntersectionObserver' in window) || reduced) return;

  var SELECTOR = '.sec-head, .card, .stat, .shot, .tl-item, .tbl-wrap, .callout, details.family';
  var targets = document.querySelectorAll(SELECTOR);
  targets.forEach(function (el) { el.classList.add('rv'); });

  var batch = [];
  function flush() {
    batch.forEach(function (el, i) {
      el.style.setProperty('--d', Math.min(i * 60, 360) + 'ms');
      el.classList.add('in');
      if (el.classList.contains('stat')) countUp(el.querySelector('.v'));
    });
    batch = [];
  }

  function countUp(el) {
    if (!el || el.dataset.counted) return;
    el.dataset.counted = '1';
    var node = el.firstChild;
    if (!node || node.nodeType !== 3) return;
    var m = node.textContent.match(/^(\d+)([\s\S]*)$/);
    if (!m) return;
    var target = parseInt(m[1], 10);
    var rest = m[2];
    var t0 = performance.now(), dur = 900;
    (function tick(t) {
      var p = Math.min(1, (t - t0) / dur);
      var e = 1 - Math.pow(1 - p, 3);
      node.textContent = Math.round(target * e) + rest;
      if (p < 1) requestAnimationFrame(tick);
      else node.textContent = target + rest;
    })(t0);
  }

  var io = new IntersectionObserver(function (entries) {
    entries.forEach(function (entry) {
      if (!entry.isIntersecting) return;
      io.unobserve(entry.target);
      batch.push(entry.target);
    });
    if (batch.length) requestAnimationFrame(flush);
  }, { threshold: 0.15, rootMargin: '0px 0px -8% 0px' });
  targets.forEach(function (el) { io.observe(el); });

  // 终端光束:仅可见时常驻(离屏即停,不持续动画)
  var term = document.querySelector('.terminal');
  if (term && 'IntersectionObserver' in window) {
    var tio = new IntersectionObserver(function (entries) {
      entries.forEach(function (en) { term.classList.toggle('live', en.isIntersecting); });
    }, { threshold: 0.1 });
    tio.observe(term);
  }
})();
