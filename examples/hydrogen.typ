#import "../lib.typ": panfig

#set page(width: auto, height: auto, margin: 12pt)

#figure(
  panfig("hydrogen.pgf", resolve: p => path(p)),
  caption: [Hydrogen atom wavefunction probability densities $|psi_(n ell m)(r, theta, phi)|^2$.],
)
