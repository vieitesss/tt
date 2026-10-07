/** Stylesheets imported as text (`?raw`), used by the motion stylesheet checks. */
declare module "*.css?raw" {
  const css: string;
  export default css;
}
