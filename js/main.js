/* Rux 主页交互:代码复制 / 截图灯箱 / 回到顶部 */
(function () {
  'use strict';

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
    window.scrollTo({ top: 0, behavior: 'smooth' });
  });
  document.body.appendChild(top);
  window.addEventListener('scroll', function () {
    top.classList.toggle('on', window.scrollY > 600);
  }, { passive: true });
})();
